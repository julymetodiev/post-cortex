// Copyright (c) 2026 Chronicle Labs Ltd.
// MIT License

// End to end against a real daemon (see daemon.ts).

import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";

import {
  Client,
  InvalidArgument,
  NotFound,
  PostCortexError,
  type RecordInput,
  Unauthenticated,
  views,
} from "../src/index.js";
import { binary, Daemon, eventually } from "./daemon.js";

const storageRecord = (title: string, store: string, extra: Partial<RecordInput> = {}): RecordInput => ({
  kind: "decision_made",
  title,
  description: `${title}, because it fits`,
  entities: [
    { name: "pcx", type: "code_component" },
    { name: store, type: "technology" },
  ],
  relations: [{ from: "pcx", to: store, type: "depends_on", context: "stores data" }],
  ...extra,
});

describe("post-cortex client", { skip: binary() ? false : "no pcx binary" }, () => {
  let daemon: Daemon;
  let pcx: Client;

  before(async () => {
    daemon = await Daemon.start();
    pcx = new Client({ target: daemon.target, token: "", author: "sdk-test" });
  });
  after(async () => {
    pcx?.close();
    await daemon?.stop();
  });

  test("health", async () => {
    const h = await pcx.health();
    assert.ok(h.healthy && h.version);
  });

  test("sessions and workspaces", async () => {
    const sid = await pcx.createSession("sdk sessions", "made by the sdk test");
    assert.equal((await pcx.getSession(sid)).name, "sdk sessions");
    assert.ok(await pcx.updateSession(sid, { name: "renamed" }));
    assert.equal((await pcx.getSession(sid)).name, "renamed");
    assert.ok((await pcx.searchSessions("renamed")).some((s) => s.id === sid));

    const ws = await pcx.createWorkspace("sdk ws", "test");
    assert.ok(await pcx.addSession(ws, sid, "primary"));
    assert.deepEqual((await pcx.getWorkspace(ws)).sessions, { [sid]: "primary" });
    assert.ok(await pcx.removeSession(ws, sid));
    assert.ok(await pcx.deleteWorkspace(ws));
    assert.ok(await pcx.deleteSession(sid));
    await assert.rejects(pcx.getSession(sid), NotFound);
  });

  test("resolve project", async () => {
    const key = "github.com/sdk-test/api";
    const first = await pcx.resolveProject(key, "api", { client: "sdk-test" });
    assert.ok(first.created);
    assert.equal(first.sessionId.length, 36);
    const again = await pcx.resolveProject(key, "other name");
    assert.deepEqual(again, { ...first, created: false });
    const ws = await pcx.getWorkspace(first.workspaceId);
    assert.equal(ws.name, "api");
    assert.deepEqual(ws.sessions, { [first.sessionId]: "primary" });

    const web = await pcx.resolveProject("github.com/sdk-test/web", "web", { workspace: "api" });
    assert.ok(web.created);
    assert.equal(web.workspaceId, first.workspaceId);
    assert.notEqual(web.sessionId, first.sessionId);
    await assert.rejects(pcx.resolveProject("has space", "x"), InvalidArgument);
  });

  test("records and validation", async () => {
    const sid = await pcx.createSession("sdk records");
    const first = await pcx.record(sid, storageRecord("Use RocksDB", "RocksDB", { confidence: 0.75 }));
    assert.equal(first.length, 36);
    const ids = await pcx.recordMany(sid, [
      storageRecord("Move to rocks-db v2", "rocks-db", { supersedes: [first] }),
      {
        kind: "problem_solved",
        title: "Compaction stall",
        description: "fixed by tuning",
        entities: [{ name: "RocksDB", type: "technology" }, { name: "compaction", type: "problem" }],
        relations: [{ from: "RocksDB", to: "compaction", type: "caused_by", context: "write amplification" }],
      },
    ]);
    assert.equal(ids.length, 2);
    const kinds = (await pcx.records(sid)).map((r) => r.kind);
    assert.equal(kinds.filter((k) => k === "decision_made").length, 2);
    assert.ok(kinds.includes("problem_solved"));

    await assert.rejects(
      pcx.record(sid, { kind: "qa", title: "no entities", entities: [], relations: [] }),
      InvalidArgument,
    );
  });

  test("graph views and upkeep", async () => {
    const sid = await pcx.createSession("sdk graph");
    const first = await pcx.record(sid, storageRecord("Use RocksDB", "RocksDB"));
    await pcx.record(sid, storageRecord("RocksDB again", "rocks_db", { supersedes: [first] }));

    const view = await eventually(async () => {
      const v = await pcx.entity("rocks db", { sessionId: sid });
      return v.totalRecords === 2 && v;
    });
    assert.equal(view.entity?.name, "RocksDB");
    assert.deepEqual(view.entity?.aliases, ["rocks_db"]);

    assert.equal(view.relations.length, 1);
    assert.equal((await pcx.impact("RocksDB", { sessionId: sid })).dependents[0].entity, "pcx");
    assert.ok((await pcx.topics({ sessionId: sid })).topics.length >= 1);
    assert.deepEqual((await pcx.radar({ sessionId: sid })).unsolvedProblems, []);
    assert.deepEqual((await pcx.fileContext("src/lib.rs", { sessionId: sid })).records, []);
    await assert.rejects(pcx.entity("nothing", { sessionId: sid }), NotFound);

    assert.ok(await pcx.forget(sid, first));
    assert.ok(!(await pcx.forget(sid, first)));
    assert.ok(await pcx.purge(sid, first));
    assert.ok(await pcx.deleteEntity(sid, "pcx"));
  });

  test("cards", async () => {
    const ws = await pcx.createWorkspace("sdk cards");
    assert.equal((await pcx.getCard({ workspaceId: ws })).card, undefined);
    const w1 = await pcx.updateCard("## Stack\nRust", { expectedVersion: 0, workspaceId: ws });
    assert.ok(w1.written);
    assert.equal(w1.card?.version, 1);
    assert.equal(w1.card?.author, "sdk-test");
    const stale = await pcx.updateCard("other", { expectedVersion: 0, workspaceId: ws });
    assert.ok(!stale.written);
    await pcx.updateCard("## Stack\nRust, SurrealDB", { expectedVersion: 1, workspaceId: ws });
    const { card, history } = await pcx.getCard({ workspaceId: ws, history: 5 });
    assert.equal(card?.version, 2);
    assert.deepEqual(history.map((c) => c.version), [1]);
  });

  test("messages", async () => {
    const sid = await pcx.createSession("sdk messages");
    const turns = [
      { turnId: "t1", role: "user" as const, text: "keep the cards short", timestamp: new Date() },
      { turnId: "t2", role: "assistant" as const, text: "done", tools: ["Edit"], files: ["CARD.md"] },
    ];
    assert.deepEqual((await pcx.appendMessages(sid, turns)).appended, [0, 1]);
    const again = await pcx.appendMessages(sid, turns);
    assert.deepEqual(again.appended, []);
    assert.equal(again.skipped, 2);
  });

  test("search and assemble", async (t) => {
    if (!(await pcx.health()).embeddingsEnabled) return t.skip("daemon without embeddings");
    const sid = await pcx.createSession("sdk search");
    const old = await pcx.record(sid, storageRecord("Storage engine is RocksDB", "RocksDB"));
    await pcx.record(
      sid,
      storageRecord("Storage engine moves to SurrealDB", "SurrealDB", { supersedes: [old], confidence: 0.75 }),
    );
    const hits = await eventually(async () => {
      const r = await pcx.search("storage engine", { sessionId: sid, mode: "fast", trace: true });
      return r.filter((h) => !h.neighbourOf).length >= 2 && r;
    }, 60_000);
    const ended = hits.find((h) => h.entryId === old)!;
    assert.equal(ended.validity?.reason, views.EndReason.SUPERSEDED);
    assert.ok(ended.validity?.validTo instanceof Date);
    assert.ok(hits.filter((h) => !h.neighbourOf).every((h) => h.signals));
    assert.ok(hits.some((h) => h.confidence === 0.75));

    const ctx = await pcx.assemble("SurrealDB", { sessionId: sid, tokenBudget: 2000 });
    assert.ok(ctx.totalTokens > 0 && ctx.formattedText.includes("SurrealDB"));
  });

  test("workspace update and archive", async () => {
    const ws = await pcx.createWorkspace("sdk lifecycle", "before");
    const other = await pcx.createWorkspace("sdk lifecycle other");
    const updated = await pcx.updateWorkspace(ws, { name: "sdk lifecycle renamed" });
    assert.equal(updated.name, "sdk lifecycle renamed");
    assert.equal(updated.description, "before");
    await assert.rejects(pcx.updateWorkspace(ws, { name: "sdk lifecycle other" }), PostCortexError);
    await assert.rejects(pcx.updateWorkspace(ws, {}), InvalidArgument);

    assert.ok(await pcx.archiveWorkspace(ws));
    assert.equal(await pcx.archiveWorkspace(ws), false);
    assert.ok((await pcx.getWorkspace(ws)).archivedAt instanceof Date);
    assert.ok((await pcx.listWorkspaces()).every((w) => w.id !== ws));
    const all = await pcx.listWorkspaces(true);
    assert.ok(all.find((w) => w.id === ws)?.archivedAt);
    assert.equal(all.find((w) => w.id === other)?.archivedAt, undefined);
    assert.ok(await pcx.unarchiveWorkspace(ws));
    assert.ok((await pcx.listWorkspaces()).some((w) => w.id === ws));
    await assert.rejects(pcx.archiveWorkspace("00000000-0000-0000-0000-000000000000"), NotFound);
    await pcx.deleteWorkspace(ws);
    await pcx.deleteWorkspace(other);
  });

  test("delete messages", async () => {
    const sid = await pcx.createSession("sdk message delete");
    await pcx.appendMessages(
      sid,
      [0, 1, 2].map((i) => ({ turnId: `d${i}`, role: "user" as const, text: `turn ${i}` })),
    );
    assert.deepEqual(await pcx.deleteMessage(sid, 1), { deleted: 1, remaining: 2 });
    assert.deepEqual(await pcx.deleteMessage(sid, 1), { deleted: 0, remaining: 2 });
    assert.deepEqual(await pcx.deleteMessages(sid), { deleted: 2, remaining: 0 });
    await assert.rejects(pcx.deleteMessages("00000000-0000-0000-0000-000000000000"), NotFound);
  });

  test("radar dismissals", async () => {
    const ws = await pcx.createWorkspace("sdk radar");
    const sid = await pcx.createSession("sdk radar");
    await pcx.addSession(ws, sid);
    await pcx.record(sid, {
      kind: "problem_solved",
      title: "Cache stampede",
      description: "still open",
      entities: [
        { name: "stampede", type: "problem" },
        { name: "cache", type: "technology" },
      ],
      relations: [{ from: "stampede", to: "cache", type: "related_to", context: "in the cache" }],
    });
    const unsolved = async (include = false): Promise<string[]> =>
      (await pcx.radar({ workspaceId: ws }, undefined, include)).unsolvedProblems.map(
        (n: { name: string }) => n.name,
      );
    await eventually(async () => (await unsolved()).length === 1);
    const d = await pcx.dismissRadarItem(ws, "unsolved_problem", "stampede");
    assert.equal(d.author, "sdk-test");
    await assert.rejects(pcx.dismissRadarItem(ws, "nope" as never, "stampede"), InvalidArgument);
    assert.deepEqual(await unsolved(), []);
    assert.equal((await pcx.radar({ sessionId: sid })).dismissed[0].key, "stampede");
    assert.deepEqual(await unsolved(true), ["stampede"]);
    assert.deepEqual((await pcx.listRadarDismissals(ws)).map((x) => x.key), ["stampede"]);
    assert.ok(await pcx.restoreRadarItem(ws, "unsolved_problem", "stampede"));
    assert.equal(await pcx.restoreRadarItem(ws, "unsolved_problem", "stampede"), false);
    assert.deepEqual(await unsolved(), ["stampede"]);
  });

  test("export and import", async () => {
    const ws = await pcx.createWorkspace("sdk export");
    const sid = await pcx.createSession("sdk export");
    await pcx.addSession(ws, sid);
    await pcx.record(sid, storageRecord("Export me", "RocksDB"));

    const archive = await pcx.export({ scope: "workspace", workspaceId: ws });
    assert.deepEqual(
      [archive.summary.sessions, archive.summary.workspaces, archive.summary.updates],
      [1, 1, 1],
    );
    assert.equal(archive.summary.totalBytes, archive.data.length);
    const zipped = await pcx.export({ scope: "session", sessionIds: [sid], compression: "zstd" });
    assert.deepEqual([...zipped.data.subarray(0, 4)], [0x28, 0xb5, 0x2f, 0xfd]);
    await assert.rejects(pcx.export({ scope: "galaxy" as never }), InvalidArgument);

    const preview = await pcx.importArchive(archive.data, { skipExisting: true, chunkSize: 512 });
    assert.equal(preview.committed, false);
    assert.deepEqual(preview.conflicts.map((c) => c.kind).sort(), ["session", "workspace"]);
    assert.ok(preview.conflicts.every((c) => c.resolution === "skip"));
    await assert.rejects(pcx.importArchive(Buffer.from("not an archive")), InvalidArgument);

    const fresh = await Daemon.start();
    const other = new Client({ target: fresh.target, token: "" });
    try {
      const done = await other.importArchive(archive.data, { commit: true, chunkSize: 512 });
      assert.ok(done.committed && done.conflicts.length === 0 && done.errors.length === 0);
      assert.deepEqual([done.sessionsImported, done.workspacesImported, done.updatesImported], [1, 1, 1]);
      assert.deepEqual((await other.getWorkspace(ws)).sessions, { [sid]: "primary" });
      assert.equal((await other.records(sid)).length, 1);

      // Diverged cards: the fresh daemon's own, then a later one here. The daemon's
      // stays unless cardsNewer takes the later one.
      await pcx.updateCard("common", { expectedVersion: 0, workspaceId: ws });
      const common = await pcx.export({ scope: "workspace", workspaceId: ws });
      await other.importArchive(common.data, { commit: true, skipExisting: true });
      await other.updateCard("mine\nmore", { expectedVersion: 1, workspaceId: ws });
      await new Promise((resolve) => setTimeout(resolve, 1100));
      await pcx.updateCard("theirs, later", { expectedVersion: 1, workspaceId: ws });
      const cards = (await pcx.export({ scope: "workspace", workspaceId: ws })).data;
      const kept = await other.importArchive(cards, { skipExisting: true });
      const card = kept.conflicts.find((c) => c.kind === "card");
      assert.equal(card?.resolution, "keep");
      assert.deepEqual([card?.target?.version, card?.target?.firstLine], [2, "mine"]);
      assert.deepEqual([card?.archive?.version, card?.archive?.firstLine], [2, "theirs, later"]);
      assert.ok(card!.archive!.updatedAt > card!.target!.updatedAt);
      const taken = await other.importArchive(cards, {
        commit: true,
        skipExisting: true,
        cardsNewer: true,
      });
      assert.deepEqual(
        taken.conflicts.filter((c) => c.kind === "card").map((c) => c.resolution),
        ["update"],
      );
      const head = (await other.getCard({ workspaceId: ws })).card;
      assert.deepEqual([head?.version, head?.text], [3, "theirs, later"]);
    } finally {
      other.close();
      await fresh.stop();
    }
  });
});

test("auth", { skip: binary() ? false : "no pcx binary" }, async () => {
  const d = await Daemon.start("s3cret");
  try {
    const anonymous = new Client({ target: d.target, token: "" });
    await assert.rejects(anonymous.health(), Unauthenticated);
    anonymous.close();
    const c = new Client({ target: d.target, token: "s3cret" });
    assert.ok((await c.health()).healthy);
    c.close();
  } finally {
    await d.stop();
  }
});
