import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { PostCortex } from "@post-cortex/plugin-core";
import { TEST_SESSION, startFakeDaemon, tempEnv, withEnv } from "@post-cortex/plugin-core/testing";

import extension from "../src/index.js";
import { AUTHOR, SECTION, register, turnsOf } from "../src/extension.js";

const noGit = async (): Promise<null> => null;

type Handler = (event: any, ctx: any) => unknown;

function fakePi(): { pi: ExtensionAPI; emit: (name: string, event: any, ctx: any) => Promise<unknown>; names: () => string[] } {
  const handlers = new Map<string, Handler[]>();
  const pi = {
    on(name: string, handler: Handler) {
      handlers.set(name, [...(handlers.get(name) ?? []), handler]);
      return () => undefined;
    },
  } as unknown as ExtensionAPI;
  return {
    pi,
    names: () => [...handlers.keys()],
    emit: async (name, event, ctx) => {
      let result: unknown;
      for (const h of handlers.get(name) ?? []) result = await h(event, ctx);
      return result;
    },
  };
}

const ctxFor = (cwd: string) => ({ cwd, sessionManager: { getSessionId: () => "pi-session-1" } });

test("default export registers the five handlers", async () => {
  const t = tempEnv("http://127.0.0.1:9");
  try {
    await withEnv({ ...t.env, PC_PROFILE: undefined, PC_AUTH_TOKEN: undefined }, async () => {
      const { pi, names } = fakePi();
      extension(pi);
      assert.deepEqual(names().sort(), ["agent_end", "before_agent_start", "session_start", "tool_call", "tool_result"]);
    });
  } finally {
    t.cleanup();
  }
});

test("cards go into the post-cortex prompt section; fetched once per cwd", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const { pi, emit } = fakePi();
    register(pi, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }));
    const ctx = ctxFor(t.projectDir);
    await emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    for (let i = 0; i < 2; i++) {
      const event = { type: "before_agent_start", messageType: "user", prompt: "hi", systemPrompt: "", systemPromptOptions: { sections: {} as Record<string, string> } };
      assert.equal(await emit("before_agent_start", event, ctx), undefined);
      assert.ok(event.systemPromptOptions.sections[SECTION]?.startsWith(`[PCX] Session: ${TEST_SESSION}`));
    }
    assert.equal(daemon.events("SessionStart").length, 1);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("a file's memory once a session: with its read, else its first edit blocked", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const { pi, emit } = fakePi();
    register(pi, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }));
    const ctx = ctxFor(t.projectDir);
    const content = [{ type: "text", text: "fn a() {}" }];
    const result = (await emit(
      "tool_result",
      { type: "tool_result", toolName: "read", toolCallId: "1", input: { path: join(t.projectDir, "src", "a.rs") }, content, isError: false, structuredContent: { ok: true } },
      ctx,
    )) as { content: Array<{ text: string }>; structuredContent: unknown };
    assert.equal(result.content.length, 2);
    assert.equal(result.content[0]?.text, "fn a() {}");
    assert.match(result.content[1]?.text ?? "", /^post-cortex memory for src\/a\.rs:/);
    assert.deepEqual(result.structuredContent, { ok: true });

    // read already: its edit goes on
    const call = (toolName: string, path: string) => ({ type: "tool_call", toolCallId: "2", toolName, input: { path } });
    assert.equal(await emit("tool_call", call("edit", "src/a.rs"), ctx), undefined);
    // never read: the first write is blocked with its memory, the second goes on
    const blocked = (await emit("tool_call", call("write", "b.rs"), ctx)) as { block: boolean; reason: string };
    assert.equal(blocked.block, true);
    assert.match(blocked.reason, /^post-cortex memory for b\.rs:/);
    assert.equal(await emit("tool_call", call("write", "b.rs"), ctx), undefined);
    assert.equal(await emit("tool_call", { ...call("bash", "c.rs"), input: { command: "ls" } }, ctx), undefined);

    for (const event of [
      { toolName: "bash", input: { command: "ls" }, isError: false },
      { toolName: "read", input: { path: "c.rs" }, isError: true },
      { toolName: "edit", input: { path: "c.rs" }, isError: false },
    ]) {
      assert.equal(await emit("tool_result", { type: "tool_result", toolCallId: "x", content, ...event }, ctx), undefined);
    }
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("capture on agent_end when enabled", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const messages = [
      { role: "user", content: "fix it", timestamp: 1_790_000_000_000 },
      {
        role: "assistant",
        content: [
          { type: "thinking", thinking: "hmm" },
          { type: "text", text: "Fixed." },
          { type: "toolCall", id: "t", name: "edit", arguments: {} },
        ],
        timestamp: 1_790_000_001_000,
      },
      { role: "toolResult", content: [{ type: "text", text: "ok" }] },
      { role: "user", content: [{ type: "text", text: "## post-cortex cards\n..." }] },
    ];
    const off = fakePi();
    register(off.pi, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }));
    await off.emit("agent_end", { type: "agent_end", messages }, ctxFor(t.projectDir));
    assert.equal(daemon.messages().length, 0);

    const on = fakePi();
    register(on.pi, new PostCortex(AUTHOR, { capture: true }, { env: t.env, origin: noGit }));
    await on.emit("agent_end", { type: "agent_end", messages }, ctxFor(t.projectDir));
    const [body] = daemon.messages();
    const sent = (body?.messages ?? []) as Array<Record<string, unknown>>;
    assert.equal(body?.session_id, TEST_SESSION);
    assert.deepEqual(
      sent.map((m) => [m.role, m.text, m.author, m.tools, m.source_session, m.timestamp]),
      [
        ["user", "fix it", null, [], "pi-session-1", "2026-09-21T14:13:20.000Z"],
        ["assistant", "Fixed.", "pi", ["edit"], "pi-session-1", "2026-09-21T14:13:21.000Z"],
      ],
    );
    assert.deepEqual(turnsOf(messages, "pi-session-1").map((m) => m.turn_id), sent.map((m) => m.turn_id));
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("disabled in hosts.pi: nothing registered", async () => {
  const t = tempEnv("http://127.0.0.1:9");
  try {
    t.writeConfig({ hosts: { pi: { enabled: false } } });
    const { pi, names } = fakePi();
    register(pi, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }));
    assert.deepEqual(names(), []);
  } finally {
    t.cleanup();
  }
});
