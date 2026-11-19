// post-cortex for OpenCode.
//
// - The ids line and the user and project cards go into the system prompt
//   of every turn (fetched once per OpenCode session).
// - What the memory knows about a file (problems solved, decisions,
//   dependents), once per OpenCode session: appended to what `read` returns,
//   else the first edit, write or apply_patch of the file is refused once
//   with it as the error the agent reads, and the same call sent again goes
//   through.
// - With capture on, finished turns go to the project's message stream when
//   the OpenCode session goes idle.
//
// The project is resolved from the directory OpenCode runs in (one plugin
// instance per directory), so a global install serves every project. Every
// daemon call has a 3 s timeout and fails open.

import { existsSync } from "node:fs";
import { join } from "node:path";

import type { Hooks, Plugin, PluginInput } from "@opencode-ai/plugin";
import { PostCortex, type Turn, isInjected, patchPaths, pluginOptions } from "@post-cortex/plugin-core";

export const AUTHOR = "opencode";

/** The files an edit tool call touches (absolute, or relative to the directory). */
export function editedFiles(tool: string, args: unknown): string[] {
  const record = typeof args === "object" && args !== null ? (args as Record<string, unknown>) : {};
  if ((tool === "edit" || tool === "write") && typeof record.filePath === "string") return [record.filePath];
  if (tool === "apply_patch" && typeof record.patchText === "string") return patchPaths(record.patchText);
  return [];
}

/** The file a `read` call reads. */
export function readFiles(tool: string, args: unknown): string[] {
  const record = typeof args === "object" && args !== null ? (args as Record<string, unknown>) : {};
  return tool === "read" && typeof record.filePath === "string" ? [record.filePath] : [];
}

type MessagesClient =Pick<PluginInput["client"], "session">;

/** The hooks for one directory; `memory` and `client` are injectable for tests. */
export function hooks(memory: PostCortex, directory: string, client: MessagesClient): Hooks {
  const cards = new Map<string, Promise<string | null>>();
  const reads = new Map<string, string[]>();
  const added = new Map<string, string>();
  const sent = new Set<string>();

  return {
    "experimental.chat.system.transform": async (input, output) => {
      const key = input.sessionID ?? "";
      let text = cards.get(key);
      if (!text) {
        text = memory.cardsText(directory);
        cards.set(key, text);
      }
      const value = await text;
      if (value) output.system.push(value);
      else cards.delete(key);
    },

    // A file's memory once per session: with its read, else holding its
    // first edit (a thrown error refuses the call; the agent reads it and
    // sends the edit again).
    "tool.execute.before": async (input, output) => {
      const read = readFiles(input.tool, output.args);
      if (read.length) {
        reads.set(input.callID, read);
        return;
      }
      const files = editedFiles(input.tool, output.args);
      if (!files.length) return;
      const found = await memory.fileMemory(directory, files, "edit", input.sessionID);
      if (found?.held) throw new Error(found.text);
      if (found) added.set(input.callID, found.text);
    },

    "tool.execute.after": async (input, output) => {
      const read = reads.get(input.callID);
      reads.delete(input.callID);
      const text = read
        ? (await memory.fileMemory(directory, read, "read", input.sessionID))?.text
        : added.get(input.callID);
      added.delete(input.callID);
      if (text) output.output += `\n\n${text}`;
    },

    event: async ({ event }) => {
      if (!memory.captureEnabled || event.type !== "session.idle") return;
      try {
        const id = event.properties.sessionID;
        const res = await client.session.messages({ path: { id } });
        const turns: Turn[] = [];
        for (const { info, parts } of res.data ?? []) {
          if (sent.has(info.id)) continue;
          const text = parts
            .flatMap((p) => (p.type === "text" && !p.synthetic ? [p.text] : []))
            .join("\n")
            .trim();
          if (!text || isInjected(text)) continue;
          turns.push({
            turn_id: info.id,
            role: info.role,
            source_session: id,
            text,
            tools: parts.flatMap((p) => (p.type === "tool" ? [p.tool] : [])),
            files: [],
          });
        }
        if (turns.length && (await memory.capture(directory, turns))) {
          for (const t of turns) sent.add(t.turn_id);
        }
      } catch {
        // fail open
      }
    },
  };
}

/** The plugin file `pcx setup --client opencode` writes into a project. */
export const PROJECT_PLUGIN = join(".opencode", "plugins", "post-cortex.ts");

/** Whether the project runs its own copy (`pcx setup --project`): project mode wins, the hooks never run twice. */
export function shadowedByProject(...roots: string[]): boolean {
  return roots.some((root) => root && existsSync(join(root, PROJECT_PLUGIN)));
}

/** The OpenCode server plugin; options come from `["@post-cortex/opencode", {...}]` in the `plugin` array. */
export const server: Plugin = async ({ client, directory, worktree }, options) => {
  if (shadowedByProject(directory, worktree)) return {};
  return hooks(new PostCortex(AUTHOR, pluginOptions(options)), directory, client);
};
