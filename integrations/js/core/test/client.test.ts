import assert from "node:assert/strict";
import { test } from "node:test";

import { PcxClient } from "../src/client.js";
import { TEST_SESSION, TEST_WORKSPACE, startFakeDaemon } from "../src/testing.js";

test("requests carry the author, the bearer token and encode their query", async () => {
  const daemon = await startFakeDaemon();
  try {
    const client = new PcxClient({ url: `${daemon.url}/`, token: " t0k ", author: "pi" });
    assert.ok(await client.cards("w 1"));
    assert.ok(await client.file("w", "src/a b.rs"));
    const [cards, file] = daemon.requests;
    assert.equal(cards?.path, "/api/cards");
    assert.deepEqual(cards?.query, { workspace_id: "w 1" });
    assert.equal(cards?.headers.authorization, "Bearer t0k");
    assert.equal(cards?.headers["x-pcx-author"], "pi");
    assert.deepEqual(file?.query, { workspace_id: "w", path: "src/a b.rs", limit: "3" });
  } finally {
    await daemon.close();
  }
});

test("no token: no Authorization header", async () => {
  const daemon = await startFakeDaemon();
  try {
    await new PcxClient({ url: daemon.url, author: "kilo" }).cards("w");
    assert.equal(daemon.requests[0]?.headers.authorization, undefined);
  } finally {
    await daemon.close();
  }
});

test("resolve sends key, name, client and the optional workspace", async () => {
  const daemon = await startFakeDaemon();
  try {
    const client = new PcxClient({ url: daemon.url, author: "opencode" });
    assert.deepEqual(await client.resolveProject({ key: "github.com/a/b", name: "b" }), {
      session_id: TEST_SESSION,
      workspace_id: TEST_WORKSPACE,
    });
    await client.resolveProject({ key: "k", name: "n", workspace: "team" });
    assert.deepEqual(daemon.requests[0]?.body, { key: "github.com/a/b", name: "b", client: "opencode" });
    assert.deepEqual(daemon.requests[1]?.body, { key: "k", name: "n", client: "opencode", workspace: "team" });
    assert.equal(daemon.requests[0]?.headers["content-type"], "application/json");
  } finally {
    await daemon.close();
  }
});

test("capture: empty turns are dropped, author only on assistant turns", async () => {
  const daemon = await startFakeDaemon();
  try {
    const client = new PcxClient({ url: daemon.url, author: "pi" });
    assert.equal(await client.capture("s", [{ turn_id: "x", role: "user", text: "  " }]), true);
    assert.equal(daemon.requests.length, 0);
    assert.equal(
      await client.capture("s", [
        { turn_id: "1", role: "user", text: "hi", source_session: "c1" },
        { turn_id: "2", role: "assistant", text: "hello", tools: ["edit"], files: ["a.rs"] },
      ]),
      true,
    );
    assert.deepEqual(daemon.messages()[0], {
      session_id: "s",
      messages: [
        { turn_id: "1", role: "user", author: null, source_session: "c1", text: "hi", tools: [], files: [] },
        { turn_id: "2", role: "assistant", author: "pi", source_session: null, text: "hello", tools: ["edit"], files: ["a.rs"] },
      ],
    });
  } finally {
    await daemon.close();
  }
});

test("fail open: HTTP errors, an unreachable daemon and timeouts are null", async () => {
  const failing = await startFakeDaemon({ status: 500 });
  const slow = await startFakeDaemon({ delayMs: 500 });
  try {
    assert.equal(await new PcxClient({ url: failing.url, author: "a" }).cards("w"), null);
    assert.equal(await new PcxClient({ url: failing.url, author: "a" }).capture("s", [{ turn_id: "1", role: "user", text: "x" }]), false);
    assert.equal(await new PcxClient({ url: failing.url, author: "a" }).resolveProject({ key: "k", name: "n" }), null);
    assert.equal(await new PcxClient({ url: "http://127.0.0.1:9", author: "a" }).cards("w"), null);
    const started = Date.now();
    assert.equal(await new PcxClient({ url: slow.url, author: "a", timeoutMs: 100 }).cards("w"), null);
    assert.ok(Date.now() - started < 450, "timed out early");
  } finally {
    await failing.close();
    await slow.close();
  }
});

test("a refused resolution is logged with the server's message; other failures are not", async () => {
  const refused = await startFakeDaemon({ status: 403 });
  const failing = await startFakeDaemon({ status: 500 });
  try {
    const lines: string[] = [];
    const log = (m: string): void => void lines.push(m);
    assert.equal(await new PcxClient({ url: refused.url, author: "a", log }).resolveProject({ key: "k", name: "n" }), null);
    assert.deepEqual(lines, ['POST /api/projects/resolve: HTTP 403: {"error":"forced"}']);
    assert.equal(await new PcxClient({ url: refused.url, author: "a", log }).cards("w"), null);
    assert.equal(await new PcxClient({ url: failing.url, author: "a", log }).resolveProject({ key: "k", name: "n" }), null);
    assert.equal(lines.length, 1);
  } finally {
    await refused.close();
    await failing.close();
  }
});

test("search posts the query and options; malformed hits are dropped; failures are null", async () => {
  const hit = { id: "e1", content_type: "DecisionPoint", title: "t", snippet: "s", score: 0.5, timestamp: "2026-10-01T00:00:00Z", author: null, session_id: "s" };
  const daemon = await startFakeDaemon({ search: () => ({ results: [hit, { id: 1 }, null] }) });
  const failing = await startFakeDaemon({ status: 400 });
  const slow = await startFakeDaemon({ delayMs: 500 });
  try {
    const client = new PcxClient({ url: daemon.url, author: "openclaw" });
    assert.deepEqual(await client.search("why rocksdb", { scope: "workspace", id: "w", mode: "fast", limit: 3 }), [hit]);
    assert.equal(daemon.requests[0]?.path, "/api/search");
    assert.deepEqual(daemon.requests[0]?.body, { query: "why rocksdb", scope: "workspace", id: "w", mode: "fast", limit: 3 });
    assert.equal(await new PcxClient({ url: failing.url, author: "a" }).search("q", { scope: "global" }), null);
    assert.equal(await new PcxClient({ url: slow.url, author: "a", timeoutMs: 100 }).search("q", { scope: "global" }), null);
  } finally {
    await daemon.close();
    await failing.close();
    await slow.close();
  }
});
