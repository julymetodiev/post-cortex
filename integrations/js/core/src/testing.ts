// Test helpers for the plugins (subpath export `@post-cortex/plugin-core/testing`):
// a fake daemon serving the five REST routes, and a temporary config dir.

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
  /** `GET /api/cards` answer (default: a user card and a project card). */
  cards?: unknown;
  /** `GET /api/file` answer by path (default: one problem for every path). */
  file?: (path: string) => unknown;
  /** `POST /api/search` answer by request body (default: {@link DEFAULT_SEARCH} for every query). */
  search?: (body: Record<string, unknown>) => unknown;
  /** Answer every request with this status instead. */
  status?: number;
  /** Delay every answer (timeouts). */
  delayMs?: number;
}

export interface FakeDaemon {
  url: string;
  requests: RecordedRequest[];
  /** Captured message bodies (POST /api/messages). */
  messages(): Array<Record<string, unknown>>;
  close(): Promise<void>;
}

export const DEFAULT_CARDS = {
  user: { text: "Prefers short answers.", version: 2 },
  workspace: { text: "Rust workspace, tokio.", version: 5 },
};

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

export const defaultFileView = (path: string): unknown => ({
  problems: [
    {
      id: "e1",
      title: `Fixed a bug in ${path}`,
      description: "The cache was not invalidated.",
      timestamp: "2026-10-01T12:00:00Z",
    },
  ],
  decisions: [],
  dependents: [{ entity: "server.rs" }],
});

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
      switch (`${req.method} ${url.pathname}`) {
        case "POST /api/projects/resolve":
          return send(200, { workspace_id: TEST_WORKSPACE, session_id: TEST_SESSION, created: true });
        case "GET /api/cards":
          return send(200, options.cards ?? DEFAULT_CARDS);
        case "GET /api/file":
          return send(200, (options.file ?? defaultFileView)(url.searchParams.get("path") ?? ""));
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
