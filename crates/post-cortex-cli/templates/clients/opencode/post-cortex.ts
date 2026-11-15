// post-cortex for OpenCode (written by `pcx setup --client opencode`; rerun
// setup to update it, local edits are replaced).
//
// - The user and project cards go into the system prompt of every turn
//   (fetched once per OpenCode session).
// - After edit, write and apply_patch, what the memory knows about the file
//   (problems solved, decisions, dependents) is appended to the tool output.
// - With capture on, finished turns go to the session's message stream when
//   the OpenCode session goes idle.
//
// Every daemon call has a 3 s timeout and any failure is ignored: memory
// never gets in the way of the agent. The token is PC_AUTH_TOKEN, else the
// one `pcx login` stored for the profile in credentials.json
// ($PC_CONFIG_DIR or ~/.post-cortex). Set PC_AUTH_TOKEN when the daemon
// requires a bearer token.

import type { Plugin } from "@opencode-ai/plugin";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, relative } from "node:path";

const DAEMON = (process.env.PC_URL?.trim() || "<DAEMON_URL>").replace(/\/+$/, "");
const PROFILE = "<PROFILE>";
const SESSION = "<SESSION_ID>";
const WORKSPACE = "<WORKSPACE_ID>";
const CAPTURE: boolean = <CAPTURE>;
const AUTHOR = "opencode";

type Json = any;

function storedToken(): string | undefined {
  try {
    const dir = process.env.PC_CONFIG_DIR?.trim() || join(homedir(), ".post-cortex");
    const creds = JSON.parse(readFileSync(join(dir, "credentials.json"), "utf8"));
    const token = creds?.[PROFILE]?.token;
    return typeof token === "string" ? token.trim() : undefined;
  } catch {
    return undefined;
  }
}

function headers(): Record<string, string> {
  const h: Record<string, string> = { "X-PCX-Author": AUTHOR };
  const token = process.env.PC_AUTH_TOKEN?.trim() || storedToken();
  if (token) h.Authorization = `Bearer ${token}`;
  return h;
}

async function call(path: string, body?: unknown): Promise<Json | null> {
  try {
    const res = await fetch(`${DAEMON}${path}`, {
      method: body === undefined ? "GET" : "POST",
      headers:
        body === undefined
          ? headers()
          : { ...headers(), "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(3000),
    });
    if (!res.ok) return null;
    return await res.json().catch(() => ({}));
  } catch {
    return null;
  }
}

const q = encodeURIComponent;

async function cardsText(): Promise<string | null> {
  const cards = await call(`/api/cards?workspace_id=${q(WORKSPACE)}`);
  if (!cards) return null;
  const parts: string[] = [];
  for (const [key, title] of [
    ["user", "User card"],
    ["workspace", "Project card"],
  ]) {
    const text = String(cards[key]?.text ?? "").trim();
    if (text) parts.push(`### ${title} (v${cards[key]?.version})\n${text}`);
  }
  if (!String(cards.workspace?.text ?? "").trim()) {
    parts.push(
      "No project card yet. Once you know the stack, conventions and current goals, " +
        "write a short one (max 2 KB): manage_workspace action update_card, workspace_id " +
        `${WORKSPACE}, expected_version 0.`,
    );
  }
  return `## post-cortex cards\n\n${parts.join("\n\n")}`;
}

function line(r: Json): string {
  const when = String(r.timestamp ?? "").slice(0, 10);
  let text = String(r.title ?? "");
  const description = String(r.description ?? "").trim();
  if (description) text += `: ${[...description].slice(0, 200).join("")}`;
  if (typeof r.validity?.valid_to === "string") {
    text += ` (no longer holds: ${r.validity?.reason ?? "ended"})`;
  }
  return `- ${when} ${text} [entry ${r.id ?? ""}]`;
}

async function fileText(path: string): Promise<string | null> {
  const view = await call(
    `/api/file?workspace_id=${q(WORKSPACE)}&path=${q(path)}&limit=3`,
  );
  if (!view) return null;
  const list = (key: string): string[] =>
    Array.isArray(view[key]) ? view[key].slice(0, 3).map(line) : [];
  const parts: string[] = [];
  const problems = list("problems");
  if (problems.length) {
    parts.push(`Problems solved here before:\n${problems.join("\n")}`);
  }
  const decisions = list("decisions");
  if (decisions.length) {
    parts.push(`Decisions and requirements about it:\n${decisions.join("\n")}`);
  }
  const dependents: string[] = Array.isArray(view.dependents)
    ? view.dependents
        .slice(0, 10)
        .map((d: Json) => d?.entity)
        .filter((e: unknown): e is string => typeof e === "string")
    : [];
  if (dependents.length) parts.push(`Depends on it: ${dependents.join(", ")}`);
  if (!parts.length) return null;
  return `post-cortex memory for ${path}:\n\n${parts.join("\n\n")}`;
}

// The files an edit tool call touches, relative to the project.
function editedFiles(tool: string, args: Json, directory: string): string[] {
  let paths: string[] = [];
  if ((tool === "edit" || tool === "write") && typeof args?.filePath === "string") {
    paths = [args.filePath];
  } else if (tool === "apply_patch" && typeof args?.patchText === "string") {
    const re = /^\*\*\* (?:Add|Update|Delete) File: (.+)$/gm;
    for (const m of args.patchText.matchAll(re)) paths.push(m[1].trim());
  }
  return [
    ...new Set(paths.map((p) => (isAbsolute(p) ? relative(directory, p) : p))),
  ];
}

export const PostCortex: Plugin = async ({ client, directory }) => {
  const cards = new Map<string, Promise<string | null>>();
  const pending = new Map<string, Promise<string[]>>();
  const sent = new Set<string>();

  return {
    "experimental.chat.system.transform": async (input, output) => {
      const key = input.sessionID ?? "";
      let text = cards.get(key);
      if (!text) {
        text = cardsText();
        cards.set(key, text);
      }
      const value = await text;
      if (value) output.system.push(value);
      else cards.delete(key);
    },

    "tool.execute.before": async (input, output) => {
      const files = editedFiles(input.tool, output.args, directory);
      if (!files.length) return;
      pending.set(
        input.callID,
        Promise.all(files.map(fileText)).then((texts) =>
          texts.filter((t): t is string => t !== null),
        ),
      );
    },

    "tool.execute.after": async (input, output) => {
      const texts = pending.get(input.callID);
      if (!texts) return;
      pending.delete(input.callID);
      const found = await texts;
      if (found.length) output.output += `\n\n${found.join("\n\n")}`;
    },

    event: async ({ event }) => {
      if (!CAPTURE || event.type !== "session.idle") return;
      try {
        const id = event.properties.sessionID;
        const res = await client.session.messages({ path: { id } });
        const messages = (res.data ?? [])
          .filter(({ info }) => !sent.has(info.id))
          .map(({ info, parts }) => ({
            turn_id: info.id,
            role: info.role,
            author: info.role === "assistant" ? AUTHOR : null,
            source_session: id,
            text: parts
              .flatMap((p) => (p.type === "text" && !p.synthetic ? [p.text] : []))
              .join("\n")
              .trim(),
            tools: parts.flatMap((p) => (p.type === "tool" ? [p.tool] : [])),
            files: [] as string[],
          }))
          .filter((m) => m.text);
        if (!messages.length) return;
        if (await call("/api/messages", { session_id: SESSION, messages })) {
          for (const m of messages) sent.add(m.turn_id);
        }
      } catch {
        // fail open
      }
    },
  };
};
