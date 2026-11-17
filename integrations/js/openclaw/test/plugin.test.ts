import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import type { OpenClawPluginApi } from "openclaw/plugin-sdk/plugin-entry";
import { PostCortex } from "@post-cortex/plugin-core";
import { NO_CARDS, TEST_SESSION, TEST_WORKSPACE, startFakeDaemon, tempEnv, withEnv } from "@post-cortex/plugin-core/testing";

import entry from "../src/index.js";
import { AUTHOR, CARDS_TTL_MS, PLUGIN_ID, register, requestOf, turnsOf } from "../src/plugin.js";

const noGit = async (): Promise<null> => null;
type Handler = (event: any, ctx: any) => unknown;

function fakeApi(pluginConfig?: Record<string, unknown>) {
  const handlers = new Map<string, Handler>();
  const api = {
    pluginConfig,
    on: (name: string, handler: Handler) => void handlers.set(name, handler),
  } as unknown as Pick<OpenClawPluginApi, "on" | "pluginConfig">;
  return { api, handlers, call: (name: string, event: any, ctx: any) => handlers.get(name)?.(event, ctx) };
}

const pkgDir = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

test("entry and manifest agree on the id; package.json points at the built entry", () => {
  const manifest = JSON.parse(readFileSync(join(pkgDir, "openclaw.plugin.json"), "utf8"));
  const pkg = JSON.parse(readFileSync(join(pkgDir, "package.json"), "utf8"));
  assert.equal(manifest.id, PLUGIN_ID);
  assert.equal((entry as { id?: string }).id, PLUGIN_ID);
  assert.equal(typeof (entry as { register?: unknown }).register, "function");
  assert.deepEqual(pkg.openclaw.extensions, ["./dist/index.js"]);
  assert.ok(pkg.files.includes("openclaw.plugin.json") && pkg.files.includes("assets/icon.png"));
  assert.deepEqual(Object.keys(manifest.configSchema.properties).sort(), ["capture", "enabled", "profile", "workspace"]);
});

test("cards are appended to the system prompt, reused within the TTL", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    let clock = 1000;
    const { api, handlers, call } = fakeApi();
    register(api, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }), () => clock);
    assert.deepEqual([...handlers.keys()].sort(), ["agent_end", "before_prompt_build"]);
    const ctx = { workspaceDir: t.projectDir, sessionId: "oc-s" };
    const first = (await call("before_prompt_build", { prompt: "hi", messages: [] }, ctx)) as { appendSystemContext: string };
    assert.ok(first.appendSystemContext.startsWith(`[PCX] Session: ${TEST_SESSION}`));
    await call("before_prompt_build", { prompt: "again", messages: [] }, ctx);
    assert.equal(daemon.events("SessionStart").length, 1);
    clock += CARDS_TTL_MS + 1;
    await call("before_prompt_build", { prompt: "later", messages: [] }, ctx);
    assert.equal(daemon.events("SessionStart").length, 2);
    assert.ok(daemon.requests.every((r) => r.headers["x-pcx-author"] === "openclaw"));
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("related memory for the request goes to the turn context; cards stay cached", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const { api, call } = fakeApi();
    register(api, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }), () => 1000);
    const ctx = { workspaceDir: t.projectDir, sessionId: "oc-s" };
    const first = (await call("before_prompt_build", { prompt: "which store do sessions use?", messages: [] }, ctx)) as Record<string, string>;
    assert.ok(first.appendSystemContext?.startsWith("[PCX] Session: "));
    assert.equal(first.appendContext, "## Related memory (post-cortex)\n\n- 2026-10-02 Use RocksDB for sessions: Embedded, no server to run. [entry r1]");
    assert.ok(!first.appendSystemContext?.includes("Related memory"), "recall stays out of the cached system prompt");

    // the harness's current request wins over the reconstructed prompt
    const second = (await call(
      "before_prompt_build",
      { prompt: "history ... and a long reconstructed prompt", currentUserMessage: "ok", messages: [] },
      ctx,
    )) as Record<string, string>;
    assert.equal(second.appendContext, undefined);
    const searches = daemon.requests.filter((r) => r.path === "/api/search");
    assert.equal(searches.length, 1);
    assert.deepEqual(searches[0]?.body, { query: "which store do sessions use?", scope: "workspace", id: TEST_WORKSPACE, mode: "fast", limit: 3 });
    assert.equal(daemon.events("SessionStart").length, 1);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("recall without cards; nothing found leaves the turn alone", async () => {
  const noCards = await startFakeDaemon({ cards: NO_CARDS });
  const t = tempEnv(noCards.url);
  try {
    const { api, call } = fakeApi();
    register(api, new PostCortex(AUTHOR, {}, { env: t.env, origin: noGit }));
    const ctx = { workspaceDir: t.projectDir };
    const result = (await call("before_prompt_build", { prompt: "what did we decide about storage?", messages: [] }, ctx)) as Record<string, string>;
    assert.ok(result.appendSystemContext?.includes("No project card yet"));
    assert.ok(result.appendContext?.startsWith("## Related memory (post-cortex)"));
  } finally {
    t.cleanup();
    await noCards.close();
  }
  const empty = await startFakeDaemon({ search: () => ({ results: [] }) });
  const t2 = tempEnv(empty.url);
  try {
    const { api, call } = fakeApi();
    register(api, new PostCortex(AUTHOR, {}, { env: t2.env, origin: noGit }));
    const result = (await call("before_prompt_build", { prompt: "what did we decide about storage?", messages: [] }, { workspaceDir: t2.projectDir })) as Record<string, string>;
    assert.equal(result.appendContext, undefined);
    assert.ok(result.appendSystemContext);
  } finally {
    t2.cleanup();
    await empty.close();
  }
  assert.equal(requestOf({ prompt: "p", currentUserMessage: "" }), "");
  assert.equal(requestOf({ prompt: "p" }), "p");
  assert.equal(requestOf({}), "");
});

test("capture on agent_end only with capture in the plugin config", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const messages = [
      { role: "user", content: "remember: we deploy on fridays", timestamp: 1_790_000_000_000 },
      { role: "assistant", content: [{ type: "text", text: "Noted." }, { type: "toolCall", name: "memory_search" }] },
      { role: "toolResult", content: "x" },
    ];
    await withEnv({ ...t.env, PC_PROFILE: undefined, PC_AUTH_TOKEN: undefined }, async () => {
      const off = fakeApi({});
      register(off.api);
      await off.call("agent_end", { messages, success: true }, { workspaceDir: t.projectDir, sessionId: "s1" });
      assert.equal(daemon.messages().length, 0);

      const on = fakeApi({ capture: true });
      register(on.api);
      await on.call("agent_end", { messages, success: true, runId: "r1" }, { workspaceDir: t.projectDir, sessionId: "s1" });
    });
    const [body] = daemon.messages();
    const sent = (body?.messages ?? []) as Array<Record<string, unknown>>;
    assert.deepEqual(sent.map((m) => [m.role, m.text, m.author, m.tools, m.source_session]), [
      ["user", "remember: we deploy on fridays", null, [], "s1"],
      ["assistant", "Noted.", "openclaw", ["memory_search"], "s1"],
    ]);
    // stable ids: the same run captured twice yields the same turn ids
    assert.deepEqual(turnsOf(messages, "s1", "r1").map((m) => m.turn_id), sent.map((m) => m.turn_id));
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("disabled in the plugin config: no hooks", () => {
  const { api, handlers } = fakeApi({ enabled: false });
  register(api);
  assert.equal(handlers.size, 0);
});

test("daemon down: no prompt change, no throw", async () => {
  const t = tempEnv("http://127.0.0.1:9");
  try {
    const { api, call } = fakeApi();
    register(api, new PostCortex(AUTHOR, { capture: true }, { env: t.env, origin: noGit }));
    assert.equal(await call("before_prompt_build", { prompt: "hi", messages: [] }, { workspaceDir: t.projectDir }), undefined);
    await call("agent_end", { messages: [{ role: "user", content: "x" }], success: true }, { workspaceDir: t.projectDir });
  } finally {
    t.cleanup();
  }
});
