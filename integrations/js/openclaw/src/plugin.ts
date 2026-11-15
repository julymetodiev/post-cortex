// post-cortex for OpenClaw.
//
// - before_prompt_build: the ids line and the user and project cards are
//   appended to the system prompt (`appendSystemContext`, cacheable),
//   refreshed at most every 10 minutes per project. The top 3 records of the
//   project's workspace for the user's request (POST /api/search, mode fast)
//   are appended to this turn's context (`appendContext`) as a short
//   "Related memory" section; requests under 12 characters get none.
// - agent_end: with capture on, the run's user and assistant turns go to the
//   project's message stream.
//
// The agent searches further through the post-cortex MCP server the manifest
// declares. The project is the agent's workspace directory
// (`ctx.workspaceDir`, ~/.openclaw/workspace by default), resolved like any
// other directory. Both hooks need
// `plugins.entries.post-cortex.hooks.allowConversationAccess: true`.

import type { OpenClawPluginApi } from "openclaw/plugin-sdk/plugin-entry";
import { PostCortex, type Turn, isInjected, pluginOptions, stableId, textOf } from "@post-cortex/plugin-core";

export const AUTHOR = "openclaw";
export const PLUGIN_ID = "post-cortex";
/** How long fetched cards are reused before they are fetched again. */
export const CARDS_TTL_MS = 10 * 60 * 1000;

/** Turns of a finished run worth capturing: user and assistant text. */
export function turnsOf(messages: readonly unknown[], sessionId: string, runId?: string): Turn[] {
  const turns: Turn[] = [];
  messages.forEach((message, index) => {
    if (typeof message !== "object" || message === null) return;
    const { role, content, timestamp, id } = message as Record<string, unknown>;
    if (role !== "user" && role !== "assistant") return;
    const text = textOf(content);
    if (!text || isInjected(text)) return;
    const tools = Array.isArray(content)
      ? content.flatMap((block: unknown) => {
          const b = block as Record<string, unknown> | null;
          return (b?.type === "toolCall" || b?.type === "tool_use") && typeof b.name === "string" ? [b.name] : [];
        })
      : [];
    const at = typeof timestamp === "number" ? timestamp : undefined;
    turns.push({
      turn_id: typeof id === "string" && id ? `openclaw:${id}` : `openclaw:${stableId(sessionId, role, at ?? `${runId ?? ""}#${index}`, text)}`,
      role,
      text,
      source_session: sessionId,
      tools,
      files: [],
      ...(at !== undefined ? { timestamp: new Date(at).toISOString() } : {}),
    });
  });
  return turns;
}

/**
 * The user's request: `currentUserMessage` when the harness gives it (an
 * empty one means no textual request), else `prompt`, which may carry
 * reconstructed history.
 */
export function requestOf(event: { prompt?: unknown; currentUserMessage?: unknown }): string {
  if (typeof event.currentUserMessage === "string") return event.currentUserMessage;
  return typeof event.prompt === "string" ? event.prompt : "";
}

type Api = Pick<OpenClawPluginApi, "on" | "pluginConfig">;

/** Register the hooks on `api`; `memory` is injectable for tests. */
export function register(api: Api, memory?: PostCortex, now: () => number = Date.now): void {
  const pc = memory ?? new PostCortex(AUTHOR, pluginOptions(api.pluginConfig));
  if (!pc.enabled) return;
  const cards = new Map<string, { at: number; text: Promise<string | null> }>();

  api.on("before_prompt_build", async (event, ctx) => {
    const dir = ctx.workspaceDir ?? process.cwd();
    let hit = cards.get(dir);
    if (!hit || now() - hit.at > CARDS_TTL_MS) {
      hit = { at: now(), text: pc.cardsText(dir) };
      cards.set(dir, hit);
    }
    const [text, recall] = await Promise.all([hit.text, pc.recallText(dir, requestOf(event))]);
    if (!text) cards.delete(dir);
    if (!text && !recall) return;
    return { ...(text ? { appendSystemContext: text } : {}), ...(recall ? { appendContext: recall } : {}) };
  });

  api.on("agent_end", async (event, ctx) => {
    if (!pc.captureEnabled || !event.messages.length) return;
    const dir = ctx.workspaceDir ?? process.cwd();
    const session = ctx.sessionId ?? ctx.sessionKey ?? "openclaw";
    await pc.capture(dir, turnsOf(event.messages, session, event.runId ?? ctx.runId));
  });
}
