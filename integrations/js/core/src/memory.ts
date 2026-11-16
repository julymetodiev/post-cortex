// The facade every plugin uses: backend + project resolution + the four
// behaviours (cards, prompt recall, file memory, capture), all lazy, memoized
// and fail-open.

import { type Backend, type Env, clean, configDir, hostSettings, resolveBackend } from "./config.js";
import { DEFAULT_TIMEOUT_MS, PcxClient, type Turn } from "./client.js";
import { cardsText, fileContextText, joinTexts, recallText } from "./format.js";
import {
  type Ids,
  type Project,
  cacheIds,
  cachedIds,
  forgetIds,
  gitOrigin,
  identity,
  pinnedIds,
  project as findProject,
  projectRelative,
} from "./project.js";
import { isInjected } from "./text.js";

/** Prompts shorter than this (trimmed) get no recall: "ok", "go on" and the like match nothing useful. */
export const MIN_RECALL_PROMPT_CHARS = 12;
/** How many records a prompt recalls. */
export const RECALL_LIMIT = 3;

/** Plugin options (client config such as OpenCode's `["@post-cortex/opencode", {...}]`); each wins over `hosts.<author>` in config.json. */
export interface PluginOptions {
  /** Backend profile (default: $PC_PROFILE, else the active one, else "cloud"). */
  profile?: string;
  /** Turn memory off for this client. */
  enabled?: boolean;
  /** Capture turns into the session's message stream (default false). */
  capture?: boolean;
  /** Workspace name a newly resolved project goes into. */
  workspace?: string;
}

/** A resolved project: its key, root and ids. */
export interface ResolvedProject extends Project, Ids {}

export interface PostCortexDeps {
  env?: Env;
  fetch?: typeof fetch;
  timeoutMs?: number;
  /** `remote.origin.url` lookup (tests). */
  origin?: (root: string) => Promise<string | null>;
  /** How long a failed resolution is remembered before retrying (default 60 s). */
  retryMs?: number;
  /** Where a refused project resolution is reported (default: stderr). */
  log?: (message: string) => void;
}

/** Read plugin options from an untyped value (a client's options object); unknown keys are ignored. */
export function pluginOptions(value: unknown): PluginOptions {
  if (typeof value !== "object" || value === null) return {};
  const raw = value as Record<string, unknown>;
  const options: PluginOptions = {};
  const profile = clean(raw.profile);
  if (profile) options.profile = profile;
  const workspace = clean(raw.workspace);
  if (workspace) options.workspace = workspace;
  if (typeof raw.enabled === "boolean") options.enabled = raw.enabled;
  if (typeof raw.capture === "boolean") options.capture = raw.capture;
  return options;
}

export class PostCortex {
  readonly author: string;
  readonly enabled: boolean;
  readonly captureEnabled: boolean;
  readonly backend: Backend | undefined;
  private readonly workspace: string | undefined;
  private readonly configDir: string;
  private readonly client: PcxClient | undefined;
  /** The identity of the backend's token (cache entries of another are misses). */
  private readonly identity: string | null;
  private readonly origin: (root: string) => Promise<string | null>;
  private readonly retryMs: number;
  private readonly projects = new Map<string, Promise<ResolvedProject | null>>();

  constructor(author: string, options: PluginOptions = {}, deps: PostCortexDeps = {}) {
    const env = deps.env ?? process.env;
    this.author = author;
    this.configDir = configDir(env);
    const host = hostSettings(author, { dir: this.configDir });
    this.enabled = options.enabled ?? host.enabled;
    this.captureEnabled = this.enabled && (options.capture ?? host.capture);
    this.workspace = options.workspace ?? host.workspace;
    this.backend = this.enabled
      ? resolveBackend({ env, dir: this.configDir, ...(options.profile ? { profile: options.profile } : {}) })
      : undefined;
    this.client = this.backend
      ? new PcxClient({
          url: this.backend.url,
          token: this.backend.token,
          author,
          timeoutMs: deps.timeoutMs ?? DEFAULT_TIMEOUT_MS,
          ...(deps.fetch ? { fetch: deps.fetch } : {}),
          ...(deps.log ? { log: deps.log } : {}),
        })
      : undefined;
    this.identity = identity(this.backend?.token);
    this.origin = deps.origin ?? gitOrigin;
    this.retryMs = deps.retryMs ?? 60_000;
  }

  /** The project `dir` is in with its ids, or null (disabled, unreachable backend). Memoized per directory. */
  project(dir: string): Promise<ResolvedProject | null> {
    if (!this.client) return Promise.resolve(null);
    const hit = this.projects.get(dir);
    if (hit) return hit;
    const value = this.resolve(dir).catch(() => null);
    this.projects.set(dir, value);
    void value.then((resolved) => {
      if (resolved) return;
      // Forget a failure after a while so a daemon started later is found.
      setTimeout(() => {
        if (this.projects.get(dir) === value) this.projects.delete(dir);
      }, this.retryMs).unref?.();
    });
    return value;
  }

  private async resolve(dir: string): Promise<ResolvedProject | null> {
    const client = this.client;
    if (!client) return null;
    const found = await findProject(dir, this.origin);
    const known = pinnedIds(found.root) ?? cachedIds(this.configDir, found.key, client.url, this.identity);
    if (known) return { ...found, ...known };
    const ids = await client.resolveProject({ key: found.key, name: found.name, workspace: this.workspace });
    if (!ids) return null;
    cacheIds(this.configDir, found.key, client.url, this.identity, ids);
    return { ...found, ...ids };
  }

  /** What to do when the backend refuses `project`'s ids: drop them from the cache when it holds them and forget the resolution of `dir`, so the next call resolves again. */
  private refused(dir: string, project: ResolvedProject): () => void {
    return () => {
      const client = this.client;
      if (client && forgetIds(this.configDir, project.key, client.url, this.identity, project)) {
        this.projects.delete(dir);
      }
    };
  }

  /** The cards text for the project at `dir` (ids line first), or null. */
  async cardsText(dir: string): Promise<string | null> {
    const project = await this.project(dir);
    if (!project || !this.client) return null;
    const cards = await this.client.cards(project.workspace_id, this.refused(dir, project));
    return cards === null ? null : cardsText(cards, project.workspace_id, project);
  }

  /**
   * Records of the project's workspace related to `prompt` (fast search, top
   * {@link RECALL_LIMIT}) as a "Related memory" section, or null (short or
   * injected prompt, nothing found, any failure).
   */
  async recallText(dir: string, prompt: string): Promise<string | null> {
    const query = prompt.trim();
    if ([...query].length < MIN_RECALL_PROMPT_CHARS || isInjected(query)) return null;
    const project = await this.project(dir);
    if (!project || !this.client) return null;
    const hits = await this.client.search(
      query,
      { scope: "workspace", id: project.workspace_id, mode: "fast", limit: RECALL_LIMIT },
      this.refused(dir, project),
    );
    return hits === null ? null : recallText(hits.slice(0, RECALL_LIMIT));
  }

  /** What the memory knows about `paths` (absolute, or relative to `dir`), or null. */
  async fileContextText(dir: string, paths: readonly string[]): Promise<string | null> {
    if (!paths.length) return null;
    const project = await this.project(dir);
    const client = this.client;
    if (!project || !client) return null;
    const relative = [...new Set(paths.map((p) => projectRelative(p, project.root, dir)))];
    const texts = await Promise.all(
      relative.map(async (path) => {
        const view = await client.file(project.workspace_id, path, undefined, this.refused(dir, project));
        return view === null ? null : fileContextText(view, path);
      }),
    );
    return joinTexts(texts);
  }

  /** Capture `turns` into the project's session when capture is on; true when stored. */
  async capture(dir: string, turns: readonly Turn[]): Promise<boolean> {
    if (!this.captureEnabled || !turns.length) return false;
    const project = await this.project(dir);
    if (!project || !this.client) return false;
    return this.client.capture(project.session_id, turns, this.refused(dir, project));
  }
}
