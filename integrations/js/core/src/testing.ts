// Test helpers for the plugins (subpath export `@post-cortex/plugin-core/testing`):
// a fake daemon serving the REST routes the plugins call, and a temporary
// config dir.

import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { type IncomingMessage, type ServerResponse, createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const TEST_SESSION = "11111111-2222-3333-4444-555555555555";
export const TEST_WORKSPACE = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

export interface RecordedRequest {
  method: string;
  path: string;
  query: Record<string, string>;
  headers: Record<string, string | string[] | undefined>;
  body: unknown;
}

export interface FakeDaemonOptions {
  /**
   * The cards a `SessionStart` event gets (`POST /api/events`), as the
   * memory module words them (default: {@link DEFAULT_CARDS}).
   */
  cards?: string;
  /**
   * What memory knows about a file, by the path of a `PostToolUse` (a read)
   * or `PreToolUse` (an edit) event (`POST /api/events`); null: nothing
   * (default: {@link defaultFileContext}). Told as the memory module tells
   * it: once per event `session_id` and file, a read's as context, an edit's
   * as a block ({@link HELD} after the text); without a session, an edit's
   * as context every time.
   */
  fileContext?: (path: string) => string | null;
  /** `POST /api/search` answer by request body (default: {@link DEFAULT_SEARCH} for every query). */
  search?: (body: Record<string, unknown>) => unknown;
  /** Answer every request with this status instead. */
  status?: number;
  /** Answer 404 to every request naming this workspace (query or body), as a backend without it does. */
  refuseWorkspace?: string;
  /** Delay every answer (timeouts). */
  delayMs?: number;
}

export interface FakeDaemon {
  url: string;
  requests: RecordedRequest[];
  /** Captured message bodies (POST /api/messages). */
  messages(): Array<Record<string, unknown>>;
  /** The events of `name` (`SessionStart`, `PreToolUse`) posted to `/api/events`. */
  events(name: string): RecordedRequest[];
  close(): Promise<void>;
}

/** The cards of a workspace with a user card and a project card, as the memory module words them. */
export const DEFAULT_CARDS =
  "## post-cortex cards\n\n### User card (v2)\nPrefers short answers.\n\n### Project card (v5)\nRust workspace, tokio.";

/** The cards of a workspace with none, as the memory module words them. */
export const NO_CARDS =
  "## post-cortex cards\n\nNo project card yet. Once you know the stack, conventions and current goals, " +
  "write a short one (max 2 KB): manage_workspace action update_card, workspace_id " +
  `${TEST_WORKSPACE}, expected_version 0.`;

export const DEFAULT_SEARCH = {
  results: [
    {
      id: "r1",
      content_type: "DecisionPoint",
      title: "Use RocksDB for sessions",
      snippet: "Embedded, no server to run.",
      score: 0.82,
      timestamp: "2026-10-02T09:00:00Z",
      author: "claude-code",
      session_id: TEST_SESSION,
    },
  ],
};

/** A file's memory as the memory module words it: one problem and one dependent. */
export const defaultFileContext = (path: string): string =>
  `post-cortex memory for ${path}:\n\n` +
  "Problems solved here before:\n" +
  `- 2026-10-01 Fixed a bug in ${path}: The cache was not invalidated. [entry e1]\n\n` +
  "Depends on it:\n- server.rs (depends on Cache)";

/** What a held edit's reason says after the file's memory (the memory module's words). */
export const HELD =
  "This is said once a session for this file, before its first edit: check your change against it, " +
  "then send the edit again (or a better one) and it goes through.";

type PostedEvent = { hook_event_name?: unknown; session_id?: unknown; tool_input?: { file_path?: unknown } };

/** The event a `POST /api/events` body carries. */
function eventOf(body: unknown): PostedEvent | undefined {
  return (body as { event?: PostedEvent } | undefined)?.event;
}

/**
 * The answer of `POST /api/events` to `body`: the cards for a session start,
 * a file's memory once per session (`told` holds `session\0path`).
 */
function eventAnswer(body: unknown, options: FakeDaemonOptions, told: Set<string>): unknown {
  const event = eventOf(body);
  const name = event?.hook_event_name;
  const context = (text: string): unknown => ({ hookSpecificOutput: { hookEventName: name, additionalContext: text } });
  if (name === "SessionStart") return context(options.cards ?? DEFAULT_CARDS);
  const path = event?.tool_input?.file_path;
  if ((name !== "PreToolUse" && name !== "PostToolUse") || typeof path !== "string") return {};
  const session = typeof event?.session_id === "string" ? event.session_id : "";
  const key = `${session}\0${path}`;
  if (session && told.has(key)) return {};
  const text = (options.fileContext ?? defaultFileContext)(path);
  if (text === null) return {};
  if (!session) return context(text);
  told.add(key);
  return name === "PreToolUse" ? { decision: "block", reason: `${text}\n\n${HELD}` } : context(text);
}

async function readBody(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  const text = Buffer.concat(chunks).toString("utf8");
  if (!text) return undefined;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/** A daemon on 127.0.0.1 with a random port. */
export async function startFakeDaemon(options: FakeDaemonOptions = {}): Promise<FakeDaemon> {
  const requests: RecordedRequest[] = [];
  const told = new Set<string>();
  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    void (async () => {
      const url = new URL(req.url ?? "/", "http://localhost");
      const body = await readBody(req);
      requests.push({
        method: req.method ?? "GET",
        path: url.pathname,
        query: Object.fromEntries(url.searchParams),
        headers: req.headers,
        body,
      });
      if (options.delayMs) await new Promise((r) => setTimeout(r, options.delayMs));
      const send = (status: number, value: unknown): void => {
        res.writeHead(status, { "Content-Type": "application/json" });
        res.end(JSON.stringify(value));
      };
      if (options.status) return send(options.status, { error: "forced" });
      const scoped = (body as { scope?: { workspace_id?: unknown } } | undefined)?.scope?.workspace_id;
      const named = url.searchParams.get("workspace_id") ?? (body as { id?: unknown } | undefined)?.id ?? scoped;
      if (options.refuseWorkspace && named === options.refuseWorkspace) return send(404, { error: "no such workspace" });
      switch (`${req.method} ${url.pathname}`) {
        case "POST /api/projects/resolve":
          return send(200, { workspace_id: TEST_WORKSPACE, session_id: TEST_SESSION, created: true });
        case "POST /api/events":
          return send(200, eventAnswer(body, options, told));
        case "POST /api/messages":
          return send(200, { appended: 1 });
        case "POST /api/search":
          return send(200, options.search ? options.search((body ?? {}) as Record<string, unknown>) : DEFAULT_SEARCH);
        default:
          return send(404, { error: "not found" });
      }
    })();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return {
    url: `http://127.0.0.1:${port}`,
    requests,
    messages: () =>
      requests
        .filter((r) => r.path === "/api/messages")
        .map((r) => r.body as Record<string, unknown>),
    events: (name: string) =>
      requests.filter((r) => r.path === "/api/events" && eventOf(r.body)?.hook_event_name === name),
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections?.();
        server.close(() => resolve());
      }),
  };
}

export interface TempEnv {
  /** Environment for the plugin: PC_CONFIG_DIR, PC_URL (when given), no profile or token. */
  env: Record<string, string>;
  configDir: string;
  /** A project directory (not a git repository). */
  projectDir: string;
  /** Write `config.json` in the config dir. */
  writeConfig(value: unknown): void;
  cleanup(): void;
}

/** A temporary config dir and project dir. */
export function tempEnv(url?: string): TempEnv {
  const base = mkdtempSync(join(tmpdir(), "pcx-plugin-"));
  const configDir = join(base, "config");
  const projectDir = join(base, "project");
  mkdirSync(configDir, { recursive: true });
  mkdirSync(projectDir, { recursive: true });
  const env: Record<string, string> = { PC_CONFIG_DIR: configDir };
  if (url) env.PC_URL = url;
  return {
    env,
    configDir,
    projectDir,
    writeConfig: (value) => writeFileSync(join(configDir, "config.json"), JSON.stringify(value)),
    cleanup: () => rmSync(base, { recursive: true, force: true }),
  };
}

/** Run `fn` with `vars` set in process.env, restoring it afterwards. */
export async function withEnv<T>(vars: Record<string, string | undefined>, fn: () => Promise<T>): Promise<T> {
  const saved: Record<string, string | undefined> = {};
  for (const [key, value] of Object.entries(vars)) {
    saved[key] = process.env[key];
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
  try {
    return await fn();
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
}
