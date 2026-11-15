// post-cortex for the Pi coding agent.
//
// - session_start: the project (from the session's cwd) and its cards are
//   fetched in the background.
// - before_agent_start: the ids line and the user and project cards go into
//   the `post-cortex` section of the structured system prompt, so Pi sends a
//   delta only when they change.
// - tool_result of `edit` and `write`: what the memory knows about the file
//   is appended to the result the model reads.
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

  pi.on("tool_result", async (event, ctx) => {
    if (!EDIT_TOOLS.has(event.toolName) || event.isError) return;
    const path = event.input.path;
    if (typeof path !== "string" || !path) return;
    const text = await memory.fileContextText(ctx.cwd, [path]);
    if (!text) return;
    return {
      content: [...event.content, { type: "text" as const, text }],
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
