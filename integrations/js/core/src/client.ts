// The daemon's REST routes the plugins use (crates/post-cortex-transport/src/
// rest.rs): POST /api/events (the cards and a file's memory, worded by the
// memory module for every client), POST /api/messages,
// POST /api/projects/resolve, POST /api/search. Every call has a timeout
// (3 s by default) and fails open: any error, timeout or non-2xx answer is
// null / false, so memory never gets in the way of the agent. A refused
// project resolution (4xx, such as a key limited to other workspaces) is
// still reported, with the server's message, through `log` (stderr by default).

import type { Ids } from "./project.js";

export const DEFAULT_TIMEOUT_MS = 3000;
/** Longest part of an error answer quoted in a log line. */
const MAX_ERROR_CHARS = 500;

/** The default `log`: one line on stderr. */
export const stderrLog = (message: string): void => {
  try {
    process.stderr.write(`post-cortex: ${message}\n`);
  } catch {
    // fail open
  }
};

/** One conversation turn to capture (POST /api/messages); captures are idempotent on `turn_id`. */
export interface Turn {
  /** The client's id for the turn. */
  turn_id: string;
  role: "user" | "assistant";
  text: string;
  /** The client's own conversation id. */
  source_session?: string | null;
  tools?: string[];
  files?: string[];
  /** RFC 3339; defaults to the time of the request. */
  timestamp?: string;
}

/** Options of a search (POST /api/search). */
export interface SearchOptions {
  scope: "session" | "workspace" | "global";
  /** The session or workspace; required unless the scope is global. */
  id?: string;
  /** Ranking effort (default: the daemon's). */
  mode?: "fast" | "local" | "accurate";
  /** Records (the daemon's default), captured messages or both. */
  sources?: "records" | "messages" | "all";
  /** 1 to 50 (daemon default 5). */
  limit?: number;
  /** Only records and messages by this author. */
  author?: string;
}

/** One search hit. */
export interface SearchHit {
  /** Entry id of a record, `message:<seq>` of a captured turn. */
  id: string;
  content_type: string;
  title: string;
  snippet: string;
  score: number;
  /** RFC 3339. */
  timestamp: string;
  author: string | null;
  session_id: string;
}

/** What the agent is doing with a file whose memory is asked for. */
export type FileMoment = "read" | "edit";

/**
 * A file's memory as the memory module answered: text added to what the
 * agent reads, or (`held`) the edit refused once with the text as the reason
 * the agent reads; the same edit sent again goes through.
 */
export interface FileMemory {
  held: boolean;
  text: string;
}

/** The context an answer of POST /api/events adds for the agent, or null. */
const addedContext = (answer: Record<string, unknown> | null): string | null => {
  const output = answer?.hookSpecificOutput;
  const text = typeof output === "object" && output !== null ? (output as Record<string, unknown>).additionalContext : undefined;
  return typeof text === "string" && text ? text : null;
};

const isHit = (value: unknown): value is SearchHit => {
  if (typeof value !== "object" || value === null) return false;
  const hit = value as Record<string, unknown>;
  return ["id", "content_type", "title", "snippet", "timestamp", "session_id"].every((k) => typeof hit[k] === "string");
};

export interface ClientOptions {
  /** Base URL without a trailing slash. */
  url: string;
  token?: string | undefined;
  /** `X-PCX-Author`: the client's name (`opencode`, `kilo`, `pi`, ...). */
  author: string;
  timeoutMs?: number;
  fetch?: typeof fetch;
  /** Where a refused project resolution is reported (default {@link stderrLog}). */
  log?: ((message: string) => void) | undefined;
}

export interface ResolveRequest {
  key: string;
  name: string;
  /** Workspace name a new key goes into (`hosts.<author>.workspace`). */
  workspace?: string | undefined;
}

/** Called when the backend refuses the ids a request names. */
export type Refused = () => void;

/** The answers that refuse a request's ids: forbidden (another organisation's, or outside the key's workspaces) and not found. */
const REFUSED = [403, 404];

export class PcxClient {
  readonly url: string;
  readonly author: string;
  private readonly token: string | undefined;
  private readonly timeoutMs: number;
  private readonly fetchFn: typeof fetch;
  private readonly log: (message: string) => void;

  constructor(options: ClientOptions) {
    this.url = options.url.replace(/\/+$/, "");
    this.author = options.author;
    this.token = options.token?.trim() || undefined;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.fetchFn = options.fetch ?? fetch;
    this.log = options.log ?? stderrLog;
  }

  private headers(json: boolean): Record<string, string> {
    const headers: Record<string, string> = { "X-PCX-Author": this.author };
    if (this.token) headers.Authorization = `Bearer ${this.token}`;
    if (json) headers["Content-Type"] = "application/json";
    return headers;
  }

  /**
   * The JSON answer of `path`, or null on any failure; with `reportRefusal`, a 4xx answer is logged with its body.
   * `refused` is called when the backend refuses the ids the request names (HTTP 403 or 404).
   */
  async request(
    method: "GET" | "POST",
    path: string,
    body?: unknown,
    reportRefusal = false,
    refused?: Refused,
  ): Promise<unknown> {
    try {
      const init: RequestInit = {
        method,
        headers: this.headers(body !== undefined),
        signal: AbortSignal.timeout(this.timeoutMs),
      };
      if (body !== undefined) init.body = JSON.stringify(body);
      const response = await this.fetchFn(`${this.url}${path}`, init);
      if (!response.ok) {
        if (refused && REFUSED.includes(response.status)) refused();
        if (reportRefusal && response.status >= 400 && response.status < 500) {
          const text = (await response.text().catch(() => "")).trim().slice(0, MAX_ERROR_CHARS);
          this.log(`${method} ${path}: HTTP ${response.status}${text ? `: ${text}` : ""}`);
        }
        return null;
      }
      const text = await response.text();
      return text ? (JSON.parse(text) as unknown) : {};
    } catch {
      return null;
    }
  }

  /**
   * The modules' merged answer to `event` in the workspace (POST
   * /api/events: the hook event, as Claude Code's JSON, with its scope);
   * null on any failure.
   */
  private async event(
    workspaceId: string,
    event: Record<string, unknown>,
    refused?: Refused,
  ): Promise<Record<string, unknown> | null> {
    const answer = await this.request(
      "POST",
      "/api/events",
      { scope: { workspace_id: workspaceId, author: this.author }, event },
      false,
      refused,
    );
    return typeof answer === "object" && answer !== null ? (answer as Record<string, unknown>) : null;
  }

  /** The user card and the project card (or how to write the project card), as a session start gets them; null on any failure. */
  async cards(workspaceId: string, refused?: Refused): Promise<string | null> {
    return addedContext(await this.event(workspaceId, { hook_event_name: "SessionStart" }, refused));
  }

  /**
   * What the memory says about `path` (project-relative) to the agent
   * session `session` (the client's own id; empty: a client that names none,
   * told on every edit) at `moment`: once per session and file, with a read,
   * or holding the first edit of a file it was not told about (docs/memory.md,
   * a file's memory). Null when nothing is to be said or on any failure.
   */
  async fileMemory(
    workspaceId: string,
    path: string,
    moment: FileMoment,
    session: string,
    refused?: Refused,
  ): Promise<FileMemory | null> {
    const answer = await this.event(
      workspaceId,
      {
        hook_event_name: moment === "read" ? "PostToolUse" : "PreToolUse",
        session_id: session,
        tool_input: { file_path: path },
      },
      refused,
    );
    if (answer?.decision === "block") {
      const reason = answer.reason;
      return { held: true, text: typeof reason === "string" && reason ? reason : "blocked by a post-cortex module" };
    }
    const text = addedContext(answer);
    return text ? { held: false, text } : null;
  }

  /** Hits for `query` (malformed ones dropped), or null on any failure. */
  async search(query: string, options: SearchOptions, refused?: Refused): Promise<SearchHit[] | null> {
    const answer = await this.request("POST", "/api/search", { query, ...options }, false, refused);
    const results = typeof answer === "object" && answer !== null ? (answer as Record<string, unknown>).results : undefined;
    return Array.isArray(results) ? results.filter(isHit) : null;
  }

  /** Store turns in the session's message stream; true when stored (or nothing to store). */
  async capture(sessionId: string, turns: readonly Turn[], refused?: Refused): Promise<boolean> {
    const messages = turns
      .filter((t) => t.text.trim())
      .map((t) => ({
        turn_id: t.turn_id,
        role: t.role,
        author: t.role === "assistant" ? this.author : null,
        source_session: t.source_session ?? null,
        text: t.text,
        tools: t.tools ?? [],
        files: t.files ?? [],
        ...(t.timestamp ? { timestamp: t.timestamp } : {}),
      }));
    if (!messages.length) return true;
    return (await this.request("POST", "/api/messages", { session_id: sessionId, messages }, false, refused)) !== null;
  }

  /** The workspace and session of a project key (created on first use), or null. */
  async resolveProject(request: ResolveRequest): Promise<Ids | null> {
    const body: Record<string, string> = { key: request.key, name: request.name, client: this.author };
    if (request.workspace) body.workspace = request.workspace;
    const answer = await this.request("POST", "/api/projects/resolve", body, true);
    if (typeof answer !== "object" || answer === null) return null;
    const { session_id, workspace_id } = answer as Record<string, unknown>;
    return typeof session_id === "string" && typeof workspace_id === "string" && session_id && workspace_id
      ? { session_id, workspace_id }
      : null;
  }
}
