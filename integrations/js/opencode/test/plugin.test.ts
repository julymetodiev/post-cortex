import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

import type { PluginInput } from "@opencode-ai/plugin";
import { PostCortex } from "@post-cortex/plugin-core";
import { TEST_SESSION, startFakeDaemon, tempEnv, withEnv } from "@post-cortex/plugin-core/testing";

import plugin from "../src/index.js";
import { AUTHOR, PROJECT_PLUGIN, editedFiles, hooks } from "../src/server.js";

const noGit = async (): Promise<null> => null;

type Client = PluginInput["client"];

function fakeClient(messages: unknown[]): Client {
  return { session: { messages: async () => ({ data: messages }) } } as unknown as Client;
}

const model = {} as never;

test("module shape: one default export that both plugin loaders accept", async () => {
  const mod = (await import("../src/index.js")) as Record<string, unknown>;
  assert.deepEqual(Object.keys(mod), ["default"]);
  // older loaders call every export as a plugin function
  for (const value of Object.values(mod)) assert.equal(typeof value, "function");
  // current loaders take a default *object* as a v1 module and otherwise call
  // each export as a legacy function (isRecord: non-null, non-array object)
  const isRecord = (v: unknown): boolean => !!v && typeof v === "object" && !Array.isArray(v);
  assert.equal(isRecord(mod.default), false);
  // the descriptor fields are there for tools that read them
  assert.equal(plugin.id, "post-cortex");
  assert.equal(typeof plugin.server, "function");
});

test("edited files: edit, write, apply_patch", () => {
  assert.deepEqual(editedFiles("edit", { filePath: "/p/a.rs" }), ["/p/a.rs"]);
  assert.deepEqual(editedFiles("write", { filePath: "b.rs", content: "" }), ["b.rs"]);
  assert.deepEqual(editedFiles("apply_patch", { patchText: "*** Begin Patch\n*** Update File: c.rs\n*** End Patch" }), ["c.rs"]);
  assert.deepEqual(editedFiles("bash", { command: "ls" }), []);
  assert.deepEqual(editedFiles("edit", null), []);
});

test("cards go into the system prompt once per session; file memory is appended after edits", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const h = hooks(new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }), t.projectDir, fakeClient([]));
    const system: string[] = [];
    await h["experimental.chat.system.transform"]?.({ sessionID: "s1", model }, { system });
    await h["experimental.chat.system.transform"]?.({ sessionID: "s1", model }, { system });
    assert.equal(system.length, 2);
    assert.ok(system[0]?.startsWith(`## post-cortex cards\n\n[PCX] Session: ${TEST_SESSION}`));
    assert.equal(daemon.requests.filter((r) => r.path === "/api/cards").length, 1);

    const call = { tool: "edit", sessionID: "s1", callID: "c1" };
    await h["tool.execute.before"]?.(call, { args: { filePath: join(t.projectDir, "src", "a.rs") } });
    const output = { title: "", output: "Edit applied.", metadata: {} };
    await h["tool.execute.after"]?.({ ...call, args: {} }, output);
    assert.match(output.output, /^Edit applied\.\n\npost-cortex memory for src\/a\.rs:\n\nProblems solved here before:/);
    assert.ok(daemon.requests.every((r) => r.headers["x-pcx-author"] === "opencode"));

    // other tools are left alone
    const bash = { title: "", output: "ok", metadata: {} };
    await h["tool.execute.before"]?.({ tool: "bash", sessionID: "s1", callID: "c2" }, { args: { command: "ls" } });
    await h["tool.execute.after"]?.({ tool: "bash", sessionID: "s1", callID: "c2", args: {} }, bash);
    assert.equal(bash.output, "ok");
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("capture on idle: text turns once each, synthetic and injected parts skipped", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const messages = [
      { info: { id: "m1", role: "user" }, parts: [{ type: "text", text: "fix the bug" }] },
      {
        info: { id: "m2", role: "assistant" },
        parts: [
          { type: "text", text: "Done.", synthetic: false },
          { type: "tool", tool: "edit" },
          { type: "text", text: "hidden", synthetic: true },
        ],
      },
      { info: { id: "m3", role: "user" }, parts: [{ type: "text", text: "<system-reminder>x</system-reminder>" }] },
    ];
    const h = hooks(new PostCortex(AUTHOR, { capture: true }, { env: t.env, origin: noGit }), t.projectDir, fakeClient(messages));
    const idle = { event: { type: "session.idle", properties: { sessionID: "oc1" } } } as never;
    await h.event?.(idle);
    await h.event?.(idle);
    const bodies = daemon.messages();
    assert.equal(bodies.length, 1);
    assert.deepEqual(bodies[0], {
      session_id: TEST_SESSION,
      messages: [
        { turn_id: "m1", role: "user", author: null, source_session: "oc1", text: "fix the bug", tools: [], files: [] },
        { turn_id: "m2", role: "assistant", author: "opencode", source_session: "oc1", text: "Done.", tools: ["edit"], files: [] },
      ],
    });
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("capture is off by default", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const h = hooks(new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }), t.projectDir, fakeClient([{ info: { id: "m", role: "user" }, parts: [{ type: "text", text: "x" }] }]));
    await h.event?.({ event: { type: "session.idle", properties: { sessionID: "oc1" } } } as never);
    assert.equal(daemon.requests.length, 0);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("server(): options from the plugin entry, project mode wins over the global install", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    await withEnv({ ...t.env, PC_PROFILE: undefined, PC_AUTH_TOKEN: undefined }, async () => {
      const input = { client: fakeClient([]), directory: t.projectDir, worktree: t.projectDir } as unknown as PluginInput;
      const h = await plugin.server(input, { capture: true });
      assert.equal(typeof h["experimental.chat.system.transform"], "function");
      mkdirSync(join(t.projectDir, ".opencode", "plugins"), { recursive: true });
      writeFileSync(join(t.projectDir, PROJECT_PLUGIN), "// project copy");
      assert.deepEqual(await plugin.server(input, {}), {});
    });
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("daemon down: nothing is added and nothing throws", async () => {
  const t = tempEnv("http://127.0.0.1:9");
  try {
    const h = hooks(new PostCortex(AUTHOR, { capture: true }, { env: t.env, origin: noGit }), t.projectDir, fakeClient([]));
    const system: string[] = [];
    await h["experimental.chat.system.transform"]?.({ sessionID: "s", model }, { system });
    assert.deepEqual(system, []);
    await h["tool.execute.before"]?.({ tool: "write", sessionID: "s", callID: "c" }, { args: { filePath: "a" } });
    const output = { title: "", output: "x", metadata: {} };
    await h["tool.execute.after"]?.({ tool: "write", sessionID: "s", callID: "c", args: {} }, output);
    assert.equal(output.output, "x");
  } finally {
    t.cleanup();
  }
});
