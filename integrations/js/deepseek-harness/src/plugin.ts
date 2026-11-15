// post-cortex for DeepSeek Harness (dsh), as a native Cordis plugin.
//
//   rules banner         static runtime context, present whenever enabled
//                         (the banner hook's per-prompt rules in Claude
//                         Code)
//   skill                post-cortex-memory (skills/, copied from the pcx
//                         templates by integrations/sync.py) registered
//                         through ctx.skills when the (optional) skills
//                         registry is composed
//   runtime context       registered once at apply; its text is the cards of
//                         the requesting agent's session (empty until fetched,
//                         and empty text contributes nothing). dsh reads it
//                         again when a request is admitted, after pre-step, so
//                         every request carries the cards once, the first one
//                         included
//   agent/pre-step        first request of each dsh session: fetch the cards
//                         (3 s bound; the waterfall is awaited); retried on the
//                         next step after a failure
//   tools/post-execute    after `edit` and `write`: append what the memory
//                         knows about the file to the result content
//   session/event         with capture on: collect user and assistant turns
//   agent/turn-stopping   flush them (serial, awaited at the turn boundary)
//   ctx.effect disposer   final flush
//
// The project is resolved from the dsh session's working directory. Tools come
// from the post-cortex MCP server (see README). Every daemon call has a 3 s
// timeout and fails open.

import type { Context } from "@deepseek-ai/cordis";
import type { PreStepDecision } from "@deepseek-ai/dsh-agent";
import type {} from "@deepseek-ai/dsh-session";
import type {} from "@deepseek-ai/dsh-skill";
import type {} from "@deepseek-ai/dsh-system-prompt";
import type {} from "@deepseek-ai/dsh-tools";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { PostCortex, type PluginOptions, type Turn, isInjected, pluginOptions, textOf } from "@post-cortex/plugin-core";

export const AUTHOR = "dsh";
export const name = "post-cortex";
/** The system prompt is required; everything else is an event. */
export const inject = ["systemPrompt"];
/** Runtime context name and sort order (runtime contexts are allocated up to 120; memory sorts last). */
export const CONTEXT_NAME = "post-cortex:cards";
export const CONTEXT_ORDER = 510;
/** The memory rules banner (what pcx-banner.py prints per prompt in Claude Code). */
export const RULES_NAME = "post-cortex:rules";
export const RULES_ORDER = 509;
export const RULES_TEXT = "[PCX] Rules: 1) Search PCX before answering code/arch questions 2) Log discoveries to PCX after";
const EDIT_TOOLS = new Set(["edit", "write"]);
/** The skill every host ships, in the package next to dist/ (integrations/sync.py copies it). */
export const SKILL_DIR = fileURLToPath(new URL("../skills/post-cortex-memory/", import.meta.url));

/** The skill in `dir`: its SKILL.md frontmatter (`name`, `description`) and body. */
export function readSkill(dir: string): { name: string; description: string; content: string } {
  const file = join(dir, "SKILL.md");
  const text = readFileSync(file, "utf8");
  const [, frontmatter, body] = /^---\n([\s\S]*?)\n---\n([\s\S]*)$/.exec(text) ?? [];
  if (frontmatter === undefined || body === undefined) throw new Error(`${file}: no frontmatter`);
  const field = (key: string): string => {
    const value = new RegExp(`^${key}: (.+)$`, "m").exec(frontmatter)?.[1];
    if (!value) throw new Error(`${file}: no ${key} in the frontmatter`);
    return value;
  };
  return { name: field("name"), description: field("description"), content: body.trimStart() };
}

/** The plugin's config row in cordis.patch.yml (all optional; each wins over `hosts.dsh` in config.json). */
export type Config = PluginOptions;

interface SessionLike {
  readonly id: string;
  readonly header?: { readonly cwd?: string };
}

const cwdOf = (session: SessionLike | undefined): string => session?.header?.cwd ?? process.cwd();

/** Wire the plugin on `ctx` with `memory` (injectable for tests). */
export function applyWith(ctx: Context, memory: PostCortex, skillDir: string = SKILL_DIR): void {
  if (!memory.enabled) return;

  // ── rules banner ─────────────────────────────────────────────────────
  // The memory rules on every request, ids or not (the banner hook's
  // fail-open shape): the text is static, so it survives fetch failures.
  ctx.systemPrompt.context({
    name: RULES_NAME,
    order: RULES_ORDER,
    interpolate: false,
    text: RULES_TEXT,
  });

  // ── skill ────────────────────────────────────────────────────────────
  // The skills registry is optional (a profile may compose no skill
  // service); register only when it is there.
  const skills = ctx.get("skills");
  if (skills) {
    const unregister = skills.register({
      ...readSkill(skillDir),
      source: "runtime",
      // references/*.md, which the body names
      resourceBase: { kind: "directory", path: skillDir },
    });
    ctx.effect(() => unregister);
  }

  // ── cards ────────────────────────────────────────────────────────────
  // Not a message added to the step: messages are durable history, so the
  // cards would come back on every later request next to the context.
  const cards = new Map<string, string>();
  const disposeContext = ctx.systemPrompt.context({
    name: CONTEXT_NAME,
    order: CONTEXT_ORDER,
    interpolate: false,
    text: (assembly) => {
      const id = assembly.agent?.session.id;
      return id === undefined ? "" : (cards.get(id) ?? "");
    },
  });
  const started = new Set<string>();

  ctx.on(
    "agent/pre-step",
    async (payload, next): Promise<PreStepDecision> => {
      const session = payload.agent.session;
      if (!started.has(session.id)) {
        started.add(session.id);
        const text = await memory.cardsText(cwdOf(session));
        if (text) cards.set(session.id, text);
        else started.delete(session.id); // try again on the next step
      }
      return next();
    },
    { prepend: true },
  );

  // ── file memory ──────────────────────────────────────────────────────
  ctx.on("tools/post-execute", async (exec, result, next) => {
    const decision = await next();
    if (result.isError || decision.kind !== "accept" || decision.value !== undefined) return decision;
    if (!EDIT_TOOLS.has(exec.name)) return decision;
    const args = exec.arguments as Record<string, unknown> | null;
    const path = typeof args?.file_path === "string" ? args.file_path : undefined;
    if (!path) return decision;
    const text = await memory.fileContextText(cwdOf(exec.agent?.session), [path]);
    if (!text) return decision;
    return { ...decision, content: [...(decision.content ?? result.content), { type: "text", text }] };
  });

  // ── capture ──────────────────────────────────────────────────────────
  if (!memory.captureEnabled) return;
  const pending = new Map<string, { cwd: string; turns: Turn[] }>();

  ctx.on("session/event", (session, event) => {
    let turn: Turn | undefined;
    if (event.type === "user/message") {
      const message = event.data;
      if (message.source.kind !== "user") return;
      const text = textOf(message.content);
      if (text && !isInjected(text)) turn = { turn_id: "", role: "user", text };
    } else if (event.type === "assistant/message") {
      const text = textOf(event.data.message.content);
      if (text) turn = { turn_id: "", role: "assistant", text };
    }
    if (!turn) return;
    const entry = pending.get(session.id) ?? { cwd: cwdOf(session), turns: [] };
    entry.turns.push({
      ...turn,
      turn_id: `dsh:${session.id}:${event.seq}`,
      source_session: session.id,
      timestamp: new Date(event.time).toISOString(),
    });
    pending.set(session.id, entry);
  });

  const flush = async (sessionId: string): Promise<void> => {
    const entry = pending.get(sessionId);
    if (!entry?.turns.length) return;
    pending.delete(sessionId);
    if (!(await memory.capture(entry.cwd, entry.turns))) {
      // keep them for the next boundary; turn ids make the retry idempotent
      const now = pending.get(sessionId);
      pending.set(sessionId, { cwd: entry.cwd, turns: [...entry.turns, ...(now?.turns ?? [])] });
    }
  };

  ctx.on("agent/turn-stopping", async (payload) => {
    await flush(payload.agent.session.id);
  });

  ctx.effect(() => async () => {
    await Promise.all([...pending.keys()].map(flush));
    disposeContext();
  });
}

/** Cordis entry: `apply(ctx, config)`. */
export function apply(ctx: Context, config: Config = {}): void {
  applyWith(ctx, new PostCortex(AUTHOR, pluginOptions(config)));
}
