import assert from "node:assert/strict";
import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

import { PostCortex, pluginOptions } from "../src/memory.js";
import { cacheIds, cachedIds, identity, localKey } from "../src/project.js";
import { isInjected, stableId, textOf } from "../src/text.js";
import { DEFAULT_CARDS, TEST_SESSION, TEST_WORKSPACE, startFakeDaemon, tempEnv } from "../src/testing.js";

const noGit = async (): Promise<null> => null;

test("resolves once per directory, caches in projects.json, then serves cards and file memory", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const pc = new PostCortex("opencode", {}, { env: t.env, origin: noGit });
    const cards = await pc.cardsText(t.projectDir);
    assert.equal(cards, `[PCX] Session: ${TEST_SESSION} | Workspace: ${TEST_WORKSPACE}\n\n${DEFAULT_CARDS}`);
    const file = await pc.fileContextText(t.projectDir, [join(t.projectDir, "src", "a.rs"), "src/a.rs"]);
    assert.ok(file?.startsWith("post-cortex memory for src/a.rs:"), file ?? "null");
    assert.equal(daemon.requests.filter((r) => r.path === "/api/projects/resolve").length, 1);
    assert.equal(daemon.events("PreToolUse").length, 1);
    const cache = JSON.parse(readFileSync(join(t.configDir, "projects.json"), "utf8"));
    const [entry] = Object.values(cache) as Array<Record<string, string>>;
    assert.deepEqual(entry, { workspace_id: TEST_WORKSPACE, session_id: TEST_SESSION, url: daemon.url });

    // a new instance reads the cache instead of resolving again
    const again = new PostCortex("opencode", {}, { env: t.env, origin: noGit });
    await again.cardsText(t.projectDir);
    assert.equal(daemon.requests.filter((r) => r.path === "/api/projects/resolve").length, 1);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("another organisation's key on the same backend resolves again; refused cached ids are dropped", async () => {
  const daemon = await startFakeDaemon({ refuseWorkspace: "W-gone" });
  const t = tempEnv(daemon.url);
  try {
    const key = localKey(realpathSync(t.projectDir));
    const gone = { session_id: "S-gone", workspace_id: "W-gone" };
    cacheIds(t.configDir, key, daemon.url, identity("key-of-my-org"), gone);
    const env = { ...t.env, PC_AUTH_TOKEN: "key-of-my-org" };

    // The new organisation's key: the entry of the other is a miss.
    const theirs = new PostCortex("pi", {}, { env: { ...t.env, PC_AUTH_TOKEN: "key-of-the-new-org" }, origin: noGit });
    assert.equal((await theirs.project(t.projectDir))?.workspace_id, TEST_WORKSPACE);
    assert.equal(daemon.requests.filter((r) => r.path === "/api/projects/resolve").length, 1);

    // The cached ids are refused (404): dropped, and the next call resolves again.
    cacheIds(t.configDir, key, daemon.url, identity("key-of-my-org"), gone);
    const pc = new PostCortex("pi", {}, { env, origin: noGit });
    assert.equal(await pc.cardsText(t.projectDir), null);
    assert.equal(cachedIds(t.configDir, key, daemon.url, identity("key-of-my-org")), null);
    const cards = await pc.cardsText(t.projectDir);
    assert.ok(cards?.includes(`Workspace: ${TEST_WORKSPACE}`), cards ?? "null");
    assert.equal(daemon.requests.filter((r) => r.path === "/api/projects/resolve").length, 2);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("pinned ids in the repository win and are not cached", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    writeFileSync(join(t.projectDir, ".post-cortex.json"), JSON.stringify({ session_id: "S", workspace_id: "W" }));
    const pc = new PostCortex("pi", {}, { env: t.env, origin: noGit });
    const project = await pc.project(t.projectDir);
    assert.equal(project?.session_id, "S");
    assert.equal(daemon.requests.length, 0);
    await pc.cardsText(t.projectDir);
    assert.deepEqual((daemon.events("SessionStart")[0]?.body as { scope?: unknown } | undefined)?.scope, {
      workspace_id: "W",
      author: "pi",
    });
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("hosts.<author>.workspace goes into the resolve request; options win over hosts", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    t.writeConfig({ hosts: { kilo: { workspace: "team", capture: true } } });
    const pc = new PostCortex("kilo", {}, { env: t.env, origin: noGit });
    assert.equal(pc.captureEnabled, true);
    await pc.project(t.projectDir);
    assert.equal((daemon.requests[0]?.body as Record<string, unknown>).workspace, "team");
    const off = new PostCortex("kilo", { capture: false }, { env: t.env, origin: noGit });
    assert.equal(off.captureEnabled, false);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("disabled: nothing is sent", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    t.writeConfig({ hosts: { pi: { enabled: false, capture: true } } });
    const pc = new PostCortex("pi", {}, { env: t.env, origin: noGit });
    assert.equal(pc.enabled, false);
    assert.equal(pc.captureEnabled, false);
    assert.equal(await pc.cardsText(t.projectDir), null);
    assert.equal(await pc.capture(t.projectDir, [{ turn_id: "1", role: "user", text: "x" }]), false);
    assert.equal(daemon.requests.length, 0);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("capture goes to the project's session only when enabled", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const off = new PostCortex("pi", {}, { env: t.env, origin: noGit });
    assert.equal(await off.capture(t.projectDir, [{ turn_id: "1", role: "user", text: "x" }]), false);
    const on = new PostCortex("pi", { capture: true }, { env: t.env, origin: noGit });
    assert.equal(await on.capture(t.projectDir, [{ turn_id: "1", role: "user", text: "x" }]), true);
    assert.equal(daemon.messages()[0]?.session_id, TEST_SESSION);
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("unreachable backend: null, and the failure is retried after retryMs", async () => {
  const t = tempEnv("http://127.0.0.1:9");
  try {
    const pc = new PostCortex("pi", {}, { env: t.env, origin: noGit, retryMs: 10 });
    assert.equal(await pc.cardsText(t.projectDir), null);
    assert.equal(await pc.fileContextText(t.projectDir, ["a"]), null);
    await new Promise((r) => setTimeout(r, 30));
    assert.equal(await pc.project(t.projectDir), null);
  } finally {
    t.cleanup();
  }
});

test("plugin options from untyped values", () => {
  assert.deepEqual(pluginOptions({ profile: " self ", capture: true, enabled: "yes", workspace: "", x: 1 }), {
    profile: "self",
    capture: true,
  });
  assert.deepEqual(pluginOptions(undefined), {});
});

test("text helpers", () => {
  assert.equal(textOf(" hi "), "hi");
  assert.equal(textOf([{ type: "text", text: "a" }, { type: "image" }, { type: "text", text: "b" }, null]), "a\nb");
  assert.equal(textOf(42), "");
  assert.equal(stableId("a", 1), stableId("a", 1));
  assert.notEqual(stableId("a", 1), stableId("a1"));
  assert.equal(stableId("x").length, 32);
  assert.ok(isInjected("<system-reminder>x"));
  assert.ok(isInjected("## post-cortex cards\n..."));
  assert.ok(!isInjected("please fix the bug"));
});

test("recall: workspace search for the prompt, fast, top 3; short and injected prompts send nothing", async () => {
  const daemon = await startFakeDaemon();
  const t = tempEnv(daemon.url);
  try {
    const pc = new PostCortex("openclaw", {}, { env: t.env, origin: noGit });
    assert.equal(await pc.recallText(t.projectDir, "  go on now  "), null);
    assert.equal(await pc.recallText(t.projectDir, "## post-cortex cards and more text"), null);
    assert.equal(daemon.requests.length, 0);
    const text = await pc.recallText(t.projectDir, " which store do sessions use? ");
    assert.ok(text?.startsWith("## Related memory (post-cortex)\n\n- 2026-10-02 Use RocksDB for sessions: Embedded"), text ?? "null");
    const search = daemon.requests.find((r) => r.path === "/api/search");
    assert.deepEqual(search?.body, { query: "which store do sessions use?", scope: "workspace", id: TEST_WORKSPACE, mode: "fast", limit: 3 });
  } finally {
    t.cleanup();
    await daemon.close();
  }
});

test("recall: nothing found or daemon down is null", async () => {
  const empty = await startFakeDaemon({ search: () => ({ results: [] }) });
  const t = tempEnv(empty.url);
  const down = tempEnv("http://127.0.0.1:9");
  try {
    assert.equal(await new PostCortex("openclaw", {}, { env: t.env, origin: noGit }).recallText(t.projectDir, "what did we decide about storage"), null);
    assert.equal(await new PostCortex("openclaw", {}, { env: down.env, origin: noGit }).recallText(down.projectDir, "what did we decide about storage"), null);
  } finally {
    t.cleanup();
    down.cleanup();
    await empty.close();
  }
});
