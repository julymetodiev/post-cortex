import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import type { Context } from "@deepseek-ai/cordis";
import { PostCortex } from "@post-cortex/plugin-core";
import { TEST_SESSION, startFakeDaemon, tempEnv, withEnv } from "@post-cortex/plugin-core/testing";

import * as entry from "../src/index.js";
import { AUTHOR, CONTEXT_NAME, RULES_NAME, RULES_TEXT, applyWith, readSkill } from "../src/plugin.js";

const noGit = async (): Promise<null> => null;
type Handler = (...args: any[]) => any;

function fakeContext(skills?: { register: (skill: unknown) => () => void }) {
  const handlers = new Map<string, Handler>();
  type Text = string | ((assembly: { agent?: { session: { id: string } } }) => string);
  const contexts: Array<{ name: string; text: Text; interpolate?: boolean; disposed: boolean }> = [];
  const effects: Array<() => unknown> = [];
  const ctx = {
    on: (name: string, handler: Handler) => {
      handlers.set(name, handler);
      return () => undefined;
    },
    get: (name: string) => (name === "skills" ? skills : undefined),
    systemPrompt: {
      context: (c: { name: string; text: Text; interpolate?: boolean }) => {
        const entry = { ...c, disposed: false };
        contexts.push(entry);
        return () => {
          entry.disposed = true;
        };
      },
    },
    effect: (execute: () => unknown) => {
      effects.push(execute);
      return { dispose: () => undefined };
    },
  } as unknown as Context;
  return {
    ctx,
    handlers,
    contexts,
    emit: (name: string, ...args: any[]) => handlers.get(name)?.(...args),
    teardown: async () => {
      for (const execute of effects) await (execute() as () => Promise<void>)();
    },
  };
}

/** The cards runtime context among the registered ones. */
const cardsContext = (f: ReturnType<typeof fakeContext>) =>
  f.contexts.find((c) => c.name === CONTEXT_NAME);

const agentIn = (cwd: string, id = "session-1") => ({ session: { id, header: { cwd } } });
const enter = (messages: unknown[] = [{ role: "user", content: [{ type: "text", text: "hi" }] }]) => async () => ({
  kind: "enter" as const,
  messages,
});

test("module exports the Cordis plugin shape; package declares the bundle", () => {
  assert.equal(entry.name, "post-cortex");
  assert.deepEqual(entry.inject, ["systemPrompt"]);
  assert.equal(typeof entry.apply, "function");
  const pkgDir = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
  const pkg = JSON.parse(readFileSync(join(pkgDir, "package.json"), "utf8"));
  assert.equal(pkg.dsh.bundle.patch, "./cordis.patch.yml");
  assert.match(readFileSync(join(pkgDir, "cordis.patch.yml"), "utf8"), /name: '@post-cortex\/dsh'/);
});

/** The context's text for a request of `sessionId`, as dsh reads it when the request is admitted. */
const textFor = (context: { text: unknown } | undefined, sessionId: string): string => {
  const text = context?.text;
  return typeof text === "function" ? text({ agent: { session: { id: sessionId } } }) : String(text ?? "");
};

test("cards reach every request once: a runtime context, never a step message", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const f = fakeContext();
    applyWith(f.ctx, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }));
    // registered up front, empty until the cards are fetched; the rules banner too
    assert.equal(f.contexts.length, 2);
    const context = cardsContext(f);
    assert.ok(context, "the cards context is registered");
    assert.equal(context?.name, CONTEXT_NAME);
    assert.equal(context?.interpolate, false);
    assert.equal(textFor(context, "session-1"), "");

    const payload = { agent: agentIn(t.projectDir), messages: [], turn: 1, step: 1 };
    const userMessage = { role: "user", content: [{ type: "text", text: "hi" }] };
    const first = await f.emit("agent/pre-step", payload, enter([userMessage]));
    assert.deepEqual(first, { kind: "enter", messages: [userMessage] }, "the step's messages are untouched");
    // admission after pre-step reads the text again: the first request has the cards
    assert.ok(textFor(context, "session-1").startsWith(`## post-cortex cards\n\n[PCX] Session: ${TEST_SESSION}`));

    // later steps: no new fetch, no message, the same single cards context
    const second = await f.emit("agent/pre-step", { ...payload, step: 2 }, enter([userMessage]));
    assert.deepEqual(second.messages, [userMessage]);
    assert.equal(f.contexts.filter((c) => c.name === CONTEXT_NAME).length, 1);
    assert.equal(daemon.requests.filter((r) => r.path === "/api/cards").length, 1);

    // another session has no cards until its own first step; a reject passes through
    assert.equal(textFor(context, "session-2"), "");
    assert.equal(textFor(context, "unknown"), "");
    const other = { ...payload, agent: agentIn(t.projectDir, "session-2") };
    assert.deepEqual(await f.emit("agent/pre-step", other, async () => ({ kind: "reject" })), { kind: "reject" });
    assert.ok(textFor(context, "session-2").startsWith("## post-cortex cards"));
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("cards fetch failed: retried on the next step", async () => {
  const t = tempEnv("http://127.0.0.1:9");
  try {
    const f = fakeContext();
    applyWith(f.ctx, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit, retryMs: 1 }));
    const payload = { agent: agentIn(t.projectDir), messages: [], turn: 1, step: 1 };
    const decision = await f.emit("agent/pre-step", payload, enter());
    assert.equal(decision.messages.length, 1);
    assert.equal(textFor(cardsContext(f), "session-1"), "");
  } finally {
    t.cleanup();
  }
  const daemon = await startFakeDaemon();
  const t2 = tempEnv(daemon.url);
  try {
    let up = false;
    const f = fakeContext();
    const fetchFn: typeof fetch = (input, init) => (up ? fetch(input, init) : Promise.reject(new Error("down")));
    applyWith(f.ctx, new PostCortex(AUTHOR, {}, { env: t2.env, origin: noGit, fetch: fetchFn, retryMs: 1 }));
    const payload = { agent: agentIn(t2.projectDir), messages: [], turn: 1, step: 1 };
    await f.emit("agent/pre-step", payload, enter());
    assert.equal(textFor(cardsContext(f), "session-1"), "");
    up = true;
    await new Promise((r) => setTimeout(r, 20)); // the failed resolution is forgotten after retryMs
    await f.emit("agent/pre-step", { ...payload, step: 2 }, enter());
    assert.ok(textFor(cardsContext(f), "session-1").startsWith("## post-cortex cards"));
  } finally {
    t2.cleanup();
    await daemon.close();
  }
});

test("file memory appended after edit and write; others, errors and values untouched", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const f = fakeContext();
    applyWith(f.ctx, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }));
    const content = [{ type: "text", text: "Wrote 3 lines." }];
    const exec = { name: "write", arguments: { file_path: join(t.projectDir, "src", "a.rs") }, agent: agentIn(t.projectDir) };
    const decision = await f.emit("tools/post-execute", exec, { isError: false, content, value: null }, async () => ({ kind: "accept" }));
    assert.equal(decision.kind, "accept");
    assert.equal(decision.content.length, 2);
    assert.match(decision.content[1].text, /^post-cortex memory for src\/a\.rs:/);

    const accept = async () => ({ kind: "accept" as const });
    assert.deepEqual(await f.emit("tools/post-execute", { ...exec, name: "bash" }, { isError: false, content }, accept), { kind: "accept" });
    assert.deepEqual(await f.emit("tools/post-execute", exec, { isError: true, content }, accept), { kind: "accept" });
    assert.deepEqual(await f.emit("tools/post-execute", exec, { isError: false, content }, async () => ({ kind: "accept", value: 1 })), { kind: "accept", value: 1 });
    assert.deepEqual(await f.emit("tools/post-execute", exec, { isError: false, content }, async () => ({ kind: "block", feedback: [] })), { kind: "block", feedback: [] });
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("capture: user and assistant turns flushed at the turn boundary, plugin messages skipped", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    await withEnv({ ...t.env, PC_PROFILE: undefined, PC_AUTH_TOKEN: undefined }, async () => {
      const f = fakeContext();
      entry.apply(f.ctx, { capture: true });
      const session = { id: "session-1", header: { cwd: t.projectDir } };
      const at = 1_790_000_000_000;
      f.emit("session/event", session, { type: "user/message", seq: 3, time: at, data: { source: { kind: "user" }, content: [{ type: "text", text: "add a test" }] } });
      f.emit("session/event", session, { type: "user/message", seq: 4, time: at, data: { source: { kind: "post-cortex" }, content: [{ type: "text", text: "cards" }] } });
      f.emit("session/event", session, { type: "tool/call", seq: 5, time: at, data: {} });
      f.emit("session/event", session, { type: "assistant/message", seq: 6, time: at + 1000, data: { message: { content: [{ type: "text", text: "Added." }] } } });
      await f.emit("agent/turn-stopping", { agent: { session }, turn: 1 });
      await f.emit("agent/turn-stopping", { agent: { session }, turn: 1 });
      await f.teardown();
    });
    const bodies = daemon.messages();
    assert.equal(bodies.length, 1);
    assert.deepEqual(bodies[0], {
      session_id: TEST_SESSION,
      messages: [
        { turn_id: "dsh:session-1:3", role: "user", author: null, source_session: "session-1", text: "add a test", tools: [], files: [], timestamp: "2026-09-21T14:13:20.000Z" },
        { turn_id: "dsh:session-1:6", role: "assistant", author: "dsh", source_session: "session-1", text: "Added.", tools: [], files: [], timestamp: "2026-09-21T14:13:21.000Z" },
      ],
    });
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("failed flush is kept and retried at teardown", async () => {
  const t = tempEnv("http://127.0.0.1:9");
  try {
    const f = fakeContext();
    applyWith(f.ctx, new PostCortex(AUTHOR, { capture: true }, { env: t.env, origin: noGit, retryMs: 1 }));
    const session = { id: "s", header: { cwd: t.projectDir } };
    f.emit("session/event", session, { type: "user/message", seq: 1, time: 0, data: { source: { kind: "user" }, content: [{ type: "text", text: "x" }] } });
    await f.emit("agent/turn-stopping", { agent: { session }, turn: 1 });
    await f.teardown(); // still down: no throw
  } finally {
    t.cleanup();
  }
});

test("disabled: nothing registered", () => {
  const t = tempEnv("http://127.0.0.1:9");
  try {
    const f = fakeContext();
    applyWith(f.ctx, new PostCortex(AUTHOR, { enabled: false }, { env: t.env, origin: noGit }));
    assert.equal(f.handlers.size, 0);
  } finally {
    t.cleanup();
  }
});

test("rules banner: static text on every request, ids or not", async () => {
  const t = tempEnv("http://127.0.0.1:9"); // unreachable backend: the rules still appear
  try {
    const f = fakeContext();
    applyWith(f.ctx, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit, retryMs: 1 }));
    const rules = f.contexts.find((c) => c.name === RULES_NAME);
    assert.ok(rules, "the rules context is registered");
    assert.equal(rules.interpolate, false);
    assert.equal(textFor(rules, "session-1"), RULES_TEXT);
    await f.emit("agent/pre-step", { agent: agentIn(t.projectDir), messages: [], turn: 1, step: 1 }, enter());
    assert.equal(textFor(rules, "session-1"), RULES_TEXT, "unchanged by a failed cards fetch");
  } finally {
    t.cleanup();
  }
});

/** The package's skills/post-cortex-memory, from build-test/test/. */
const SKILL = fileURLToPath(new URL("../../skills/post-cortex-memory/", import.meta.url));
/** The pcx template it is copied from. */
const TEMPLATE = fileURLToPath(
  new URL("../../../../../crates/post-cortex-cli/templates/skills/post-cortex-memory/SKILL.md", import.meta.url),
);

test("skill: the package ships the pcx template's skill", () => {
  assert.equal(readFileSync(join(SKILL, "SKILL.md"), "utf8"), readFileSync(TEMPLATE, "utf8"));
  const skill = readSkill(SKILL);
  assert.equal(skill.name, "post-cortex-memory");
  assert.match(skill.description, /^Long-term project memory/);
  assert.match(skill.content, /^# post-cortex memory\n/);
});

test("skill: registered when the skills registry is composed, unregistered on teardown", async () => {
  const registered: unknown[] = [];
  let unregistered = 0;
  const f = fakeContext({
    register: (skill: unknown) => {
      registered.push(skill);
      return () => {
        unregistered += 1;
      };
    },
  });
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    applyWith(f.ctx, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }), SKILL);
    assert.deepEqual(registered, [
      { ...readSkill(SKILL), source: "runtime", resourceBase: { kind: "directory", path: SKILL } },
    ]);
    assert.equal(unregistered, 0);
    await f.teardown();
    assert.equal(unregistered, 1);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("skill: nothing registered when no skills service is composed", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const f = fakeContext(undefined); // ctx.get('skills') is undefined
    applyWith(f.ctx, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }));
    await f.emit("agent/pre-step", { agent: agentIn(t.projectDir), messages: [], turn: 1, step: 1 }, enter());
    assert.ok(cardsContext(f), "the plugin still works without the skill registry");
  } finally {
    t.cleanup();
    await daemon.close();
  }
});
