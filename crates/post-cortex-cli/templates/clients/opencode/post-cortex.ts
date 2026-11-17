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

// The text the memory module adds for a hook `event` in the workspace, the
// same for every client: the cards for a session start, a file's memory
// before an edit.
async function memoryText(event: Json): Promise<string | null> {
  const answer = await call("/api/events", {
    scope: { workspace_id: WORKSPACE, author: AUTHOR },
    event,
  });
  const text = answer?.hookSpecificOutput?.additionalContext;
  return typeof text === "string" && text ? text : null;
}

const cardsText = (): Promise<string | null> => memoryText({ hook_event_name: "SessionStart" });

const fileText = (path: string): Promise<string | null> =>
  memoryText({ hook_event_name: "PreToolUse", tool_input: { file_path: path } });

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
