// post-cortex for the Pi coding agent.
//
// - session_start: the project (from the session's cwd) and its cards are
//   fetched in the background.
// - before_agent_start: the ids line and the user and project cards go into
//   the `post-cortex` section of the structured system prompt, so Pi sends a
//   delta only when they change.
// - tool_result of `read`: what the memory knows about the file is appended
//   to the result the model reads, once a session.
// - tool_call of `edit` and `write` of a file the session was not told
//   about: blocked once with that memory as the reason; the same call sent
//   again goes through.
// - agent_end: with capture on, the run's user and assistant turns go to the
//   project's message stream.
//
// Tools come from the post-cortex MCP server in Pi's mcp.json (see README);
// this extension registers none. Every daemon call has a 3 s timeout and
// fails open.

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { PostCortex, type PluginOptions, type Turn, isInjected, stableId, textOf } from "@post-cortex/plugin-core";

export const AUTHOR = "pi";
/** The system prompt section the cards go into (an XML tag name). */
export const SECTION = "post-cortex";
const EDIT_TOOLS = new Set(["edit", "write"]);
const READ_TOOLS = new Set(["read"]);

/** The file a read, edit or write call names (`path`). */
const pathOf = (input: unknown): string | undefined => {
  const path = typeof input === "object" && input !== null ? (input as Record<string, unknown>).path : undefined;
  return typeof path === "string" && path ? path : undefined;
};

/** Turns of the run's messages worth capturing: user and assistant text, harness and post-cortex text skipped. */
export function turnsOf(messages: readonly unknown[], sessionId: string): Turn[] {
  const turns: Turn[] = [];
  for (const message of messages) {
    if (typeof message !== "object" || message === null) continue;
    const { role, content, timestamp } = message as Record<string, unknown>;
    if (role !== "user" && role !== "assistant") continue;
    const text = textOf(content);
    if (!text || isInjected(text)) continue;
    const tools = Array.isArray(content)
      ? content.flatMap((block: unknown) => {
          const b = block as Record<string, unknown> | null;
          return b?.type === "toolCall" && typeof b.name === "string" ? [b.name] : [];
        })
      : [];
    const at = typeof timestamp === "number" ? timestamp : undefined;
    turns.push({
      turn_id: `pi:${stableId(sessionId, role, at, text)}`,
      role,
      text,
      source_session: sessionId,
      tools,
      files: [],
      ...(at !== undefined ? { timestamp: new Date(at).toISOString() } : {}),
    });
  }
  return turns;
}

/** Register the post-cortex handlers on `pi`; `memory` is injectable for tests. */
export function register(pi: ExtensionAPI, memory: PostCortex): void {
  if (!memory.enabled) return;
  const cards = new Map<string, Promise<string | null>>();
  const cardsFor = (cwd: string): Promise<string | null> => {
    let text = cards.get(cwd);
    if (!text) {
      text = memory.cardsText(cwd).then((value) => {
        if (value === null) cards.delete(cwd); // retry on the next run
        return value;
      });
      cards.set(cwd, text);
    }
    return text;
  };

  pi.on("session_start", (_event, ctx) => {
    void cardsFor(ctx.cwd);
  });

  pi.on("before_agent_start", async (event, ctx) => {
    const text = await cardsFor(ctx.cwd);
    if (text) event.systemPromptOptions.sections[SECTION] = text;
  });

  // A file's memory once per Pi session: with its read, else the first edit
  // or write of it is blocked once with the memory as the reason the model
  // reads, and the same call sent again goes through.
  pi.on("tool_call", async (event, ctx) => {
    const path = EDIT_TOOLS.has(event.toolName) ? pathOf(event.input) : undefined;
    if (!path) return;
    const found = await memory.fileMemory(ctx.cwd, [path], "edit", ctx.sessionManager.getSessionId());
    if (found?.held) return { block: true, reason: found.text };
  });

  pi.on("tool_result", async (event, ctx) => {
    const path = READ_TOOLS.has(event.toolName) && !event.isError ? pathOf(event.input) : undefined;
    if (!path) return;
    const found = await memory.fileMemory(ctx.cwd, [path], "read", ctx.sessionManager.getSessionId());
    if (!found) return;
    return {
      content: [...event.content, { type: "text" as const, text: found.text }],
      ...(event.structuredContent !== undefined ? { structuredContent: event.structuredContent } : {}),
    };
  });

  pi.on("agent_end", async (event, ctx) => {
    if (!memory.captureEnabled) return;
    const turns = turnsOf(event.messages, ctx.sessionManager.getSessionId());
    await memory.capture(ctx.cwd, turns);
  });
}

/** The extension factory with options (profile, capture, workspace, enabled). */
export function postCortex(options: PluginOptions = {}): (pi: ExtensionAPI) => void {
  return (pi) => register(pi, new PostCortex(AUTHOR, options));
}
