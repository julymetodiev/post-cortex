# Copyright (c) 2026 Chronicle Labs Ltd.
# MIT License
"""End to end against a real daemon (see conftest.py)."""

from __future__ import annotations

import time
from typing import Optional
from datetime import datetime, timezone

import pytest

from post_cortex import (
    Client,
    Entity,
    InvalidArgument,
    Message,
    NotFound,
    PostCortexError,
    Record,
    Relation,
    Unauthenticated,
    proto,
    views,
)

from .conftest import Daemon, eventually


def storage_record(
    title: str, store: str, *, supersedes: Optional[list[str]] = None, confidence: Optional[float] = None
) -> Record:
    return Record(
        "decision_made",
        title,
        f"{title}, because it fits",
        entities=[Entity("pcx", "code_component"), Entity(store, "technology")],
        relations=[Relation("pcx", store, "depends_on", "stores data")],
        supersedes=supersedes or [],
        confidence=confidence,
    )


def test_health(pcx: Client) -> None:
    h = pcx.health()
    assert h.healthy and h.version


def test_sessions_and_workspaces(pcx: Client) -> None:
    sid = pcx.create_session("sdk sessions", "made by the sdk test")
    assert pcx.get_session(sid).name == "sdk sessions"
    assert pcx.update_session(sid, name="renamed")
    assert pcx.get_session(sid).name == "renamed"
    assert any(s.id == sid for s in pcx.search_sessions("renamed"))
    assert any(s.id == sid for s in pcx.list_sessions())

    ws = pcx.create_workspace("sdk ws", "test")
    assert pcx.add_session(ws, sid, "primary")
    assert pcx.get_workspace(ws).sessions == {sid: "primary"}
    assert pcx.remove_session(ws, sid)
    assert pcx.get_workspace(ws).sessions == {}
    assert pcx.delete_workspace(ws)
    assert pcx.delete_session(sid)
    with pytest.raises(NotFound):
        pcx.get_session(sid)


def test_resolve_project(pcx: Client) -> None:
    key = "github.com/sdk-test/api"
    first = pcx.resolve_project(key, "api", client="sdk-test")
    assert first.created and len(first.session_id) == 36
    again = pcx.resolve_project(key, "other name")
    assert (again.workspace_id, again.session_id, again.created) == (
        first.workspace_id,
        first.session_id,
        False,
    )
    ws = pcx.get_workspace(first.workspace_id)
    assert ws.name == "api" and ws.sessions == {first.session_id: "primary"}

    web = pcx.resolve_project("github.com/sdk-test/web", "web", workspace="api")
    assert web.created and web.workspace_id == first.workspace_id
    assert web.session_id != first.session_id
    with pytest.raises(InvalidArgument):
        pcx.resolve_project("has space", "x")


def test_records_and_validation(pcx: Client) -> None:
    sid = pcx.create_session("sdk records")
    first = pcx.record(sid, storage_record("Use RocksDB", "RocksDB", confidence=0.75))
    assert len(first) == 36
    ids = pcx.record_many(
        sid,
        [
            storage_record("Move to rocks-db v2", "rocks-db", supersedes=[first]),
            Record(
                "problem_solved",
                "Compaction stall",
                "fixed by tuning",
                entities=[Entity("RocksDB", "technology"), Entity("compaction", "problem")],
                relations=[Relation("RocksDB", "compaction", "caused_by", "write amplification")],
            ),
        ],
    )
    assert len(ids) == 2
    kinds = [r.interaction_type for r in pcx.records(sid)]
    assert kinds.count("decision_made") == 2 and "problem_solved" in kinds

    with pytest.raises(InvalidArgument):
        pcx.record(sid, Record("qa", "no entities", "x"))
    with pytest.raises(InvalidArgument):
        pcx.record(sid, storage_record("bad supersedes", "x", supersedes=["00000000-0000-0000-0000-000000000000"]))


def test_graph_views_and_upkeep(pcx: Client) -> None:
    sid = pcx.create_session("sdk graph")
    first = pcx.record(sid, storage_record("Use RocksDB", "RocksDB"))
    pcx.record(sid, storage_record("RocksDB again", "rocks_db", supersedes=[first]))

    # The graph merge runs on the daemon's background pipeline.
    view = eventually(
        lambda: v if (v := pcx.entity("rocks db", session_id=sid)).total_records == 2 else None
    )
    assert view.entity.name == "RocksDB"
    assert list(view.entity.aliases) == ["rocks_db"]

    assert pcx.impact("RocksDB", session_id=sid).dependents[0].entity == "pcx"
    assert len(view.relations) == 1
    assert len(pcx.topics(session_id=sid).topics) >= 1
    assert list(pcx.radar(session_id=sid).unsolved_problems) == []
    assert pcx.lineage("RocksDB", session_id=sid).entity == "RocksDB"
    assert list(pcx.file_context("src/lib.rs", session_id=sid).records) == []
    with pytest.raises(NotFound):
        pcx.entity("nothing", session_id=sid)

    assert pcx.forget(sid, first)
    assert not pcx.forget(sid, first), "already retired"
    assert pcx.purge(sid, first)
    assert not pcx.purge(sid, first)
    assert not pcx.merge_entities(sid, "pcx", "pcx")
    assert pcx.delete_entity(sid, "pcx")


def test_cards(pcx: Client) -> None:
    ws = pcx.create_workspace("sdk cards")
    card, history = pcx.get_card(workspace_id=ws)
    assert card is None and history == []
    w1 = pcx.update_card("## Stack\nRust", expected_version=0, workspace_id=ws)
    assert w1.written and w1.card is not None
    assert w1.card.version == 1 and w1.card.author == "sdk-test"
    stale = pcx.update_card("other", expected_version=0, workspace_id=ws)
    assert not stale.written and stale.card is not None and stale.card.version == 1
    pcx.update_card("## Stack\nRust, SurrealDB", expected_version=1, workspace_id=ws)
    card, history = pcx.get_card(workspace_id=ws, history=5)
    assert card is not None and card.version == 2 and [c.version for c in history] == [1]


def test_messages(pcx: Client) -> None:
    sid = pcx.create_session("sdk messages")
    turns = [
        Message("t1", "user", "keep the cards short", timestamp=datetime.now(timezone.utc)),
        Message("t2", "assistant", "done", tools=["Edit"], files=["CARD.md"]),
    ]
    first = pcx.append_messages(sid, turns)
    assert list(first.appended) == [0, 1]
    again = pcx.append_messages(sid, turns)
    assert list(again.appended) == [] and again.skipped == 2


def test_search_and_assemble(pcx: Client) -> None:
    if not pcx.health().embeddings_enabled:
        pytest.skip("daemon built without embeddings")
    sid = pcx.create_session("sdk search")
    old = pcx.record(sid, storage_record("Storage engine is RocksDB", "RocksDB"))
    pcx.record(
        sid,
        storage_record("Storage engine moves to SurrealDB", "SurrealDB", supersedes=[old], confidence=0.75),
    )

    def found() -> Optional[list[proto.SearchResult]]:
        hits = pcx.search("storage engine", session_id=sid, mode="fast", trace=True)
        return hits if len([h for h in hits if not h.HasField("neighbour_of")]) >= 2 else None

    hits = eventually(found, timeout=60)
    by_id = {h.entry_id: h for h in hits}
    ended = by_id[old]
    assert ended.validity.reason == views.SUPERSEDED and ended.validity.HasField("valid_to")
    assert all(h.HasField("signals") for h in hits if not h.HasField("neighbour_of"))
    assert any(h.confidence == 0.75 for h in hits)

    ctx = pcx.assemble("SurrealDB", session_id=sid, token_budget=2000)
    assert ctx.total_tokens > 0 and "SurrealDB" in ctx.formatted_text


def test_auth() -> None:
    d = Daemon(token="s3cret")
    try:
        with Client(d.target, token="") as anonymous, pytest.raises(Unauthenticated):
            anonymous.health()
        with Client(d.target, token="s3cret") as c:
            assert c.health().healthy
    finally:
        d.stop()


def test_workspace_update_and_archive(pcx: Client) -> None:
    ws = pcx.create_workspace("sdk lifecycle", "before")
    other = pcx.create_workspace("sdk lifecycle other")
    updated = pcx.update_workspace(ws, name="sdk lifecycle renamed")
    assert (updated.name, updated.description) == ("sdk lifecycle renamed", "before")
    assert pcx.update_workspace(ws, description="after").description == "after"
    with pytest.raises(PostCortexError) as taken:
        pcx.update_workspace(ws, name="sdk lifecycle other")
    assert taken.value.code.name == "ALREADY_EXISTS"
    with pytest.raises(InvalidArgument):
        pcx.update_workspace(ws)

    assert pcx.archive_workspace(ws)
    assert not pcx.archive_workspace(ws), "already archived"
    assert pcx.get_workspace(ws).archived
    assert all(w.id != ws for w in pcx.list_workspaces())
    listed = {w.id: w for w in pcx.list_workspaces(include_archived=True)}
    assert listed[ws].archived_at is not None and not listed[other].archived
    assert pcx.unarchive_workspace(ws)
    assert any(w.id == ws for w in pcx.list_workspaces())
    with pytest.raises(NotFound):
        pcx.archive_workspace("00000000-0000-0000-0000-000000000000")
    assert pcx.delete_workspace(ws) and pcx.delete_workspace(other)


def test_delete_messages(pcx: Client) -> None:
    sid = pcx.create_session("sdk message delete")
    pcx.append_messages(sid, [Message(f"d{i}", "user", f"turn {i}") for i in range(3)])
    r = pcx.delete_message(sid, 1)
    assert (r.deleted, r.remaining) == (1, 2)
    r = pcx.delete_message(sid, 1)
    assert (r.deleted, r.remaining) == (0, 2)
    r = pcx.delete_messages(sid)
    assert (r.deleted, r.remaining) == (2, 0)
    with pytest.raises(NotFound):
        pcx.delete_messages("00000000-0000-0000-0000-000000000000")


def test_radar_dismissals(pcx: Client) -> None:
    ws = pcx.create_workspace("sdk radar")
    sid = pcx.create_session("sdk radar")
    pcx.add_session(ws, sid)
    pcx.record(
        sid,
        Record(
            "problem_solved",
            "Cache stampede",
            "still open",
            entities=[Entity("stampede", "problem"), Entity("cache", "technology")],
            relations=[Relation("stampede", "cache", "related_to", "in the cache")],
        ),
    )

    def unsolved(include_dismissed: bool = False) -> list[str]:
        return [
            n.name
            for n in pcx.radar(workspace_id=ws, include_dismissed=include_dismissed).unsolved_problems
        ]

    eventually(lambda: unsolved() == ["stampede"])
    d = pcx.dismiss_radar_item(ws, "unsolved_problem", "stampede")
    assert (d.kind, d.key, d.author) == ("unsolved_problem", "stampede", "sdk-test")
    with pytest.raises(InvalidArgument):
        pcx.dismiss_radar_item(ws, "nope", "stampede")
    assert unsolved() == []
    assert pcx.radar(session_id=sid).dismissed[0].key == "stampede"
    assert unsolved(include_dismissed=True) == ["stampede"]
    assert [x.key for x in pcx.list_radar_dismissals(ws)] == ["stampede"]
    assert pcx.restore_radar_item(ws, "unsolved_problem", "stampede")
    assert not pcx.restore_radar_item(ws, "unsolved_problem", "stampede")
    assert unsolved() == ["stampede"]


def test_export_and_import(pcx: Client) -> None:
    ws = pcx.create_workspace("sdk export")
    sid = pcx.create_session("sdk export")
    pcx.add_session(ws, sid)
    pcx.record(sid, storage_record("Export me", "RocksDB"))

    archive = pcx.export("workspace", workspace_id=ws)
    s = archive.summary
    assert (s.sessions, s.workspaces, s.updates) == (1, 1, 1)
    assert s.total_bytes == len(archive.data) and archive.data.startswith(b"{")
    zipped = pcx.export("session", session_ids=[sid], compression="gzip")
    assert zipped.data[:2] == b"\x1f\x8b" and zipped.summary.workspaces == 0
    with pytest.raises(InvalidArgument):
        pcx.export("galaxy")

    # Into the same daemon: a preview, with both ids in conflict.
    preview = pcx.import_archive(archive.data, skip_existing=True, chunk_size=512)
    assert not preview.committed
    assert (preview.sessions, preview.workspaces, preview.updates) == (1, 1, 1)
    assert sorted(c.kind for c in preview.conflicts) == ["session", "workspace"]
    assert all(c.resolution == "skip" for c in preview.conflicts)
    skipped = pcx.import_archive(archive.data, commit=True, skip_existing=True)
    assert skipped.committed and (skipped.sessions_skipped, skipped.workspaces_skipped) == (1, 1)
    with pytest.raises(InvalidArgument):
        pcx.import_archive(b"not an archive")

    # Into a fresh daemon: everything comes over.
    fresh = Daemon()
    try:
        with Client(fresh.target, token="") as other:
            done = other.import_archive(archive.data, commit=True, chunk_size=512)
            assert done.committed and not done.conflicts and not done.errors
            assert (done.sessions_imported, done.workspaces_imported, done.updates_imported) == (1, 1, 1)
            assert other.get_workspace(ws).sessions == {sid: "primary"}
            assert len(other.records(sid)) == 1
            again = other.export("workspace", workspace_id=ws).summary
            assert (again.sessions, again.workspaces, again.updates) == (1, 1, 1)

            # Diverged cards: the fresh daemon's own, then a later one here. The
            # daemon's stays unless cards_newer takes the later one.
            pcx.update_card("common", expected_version=0, workspace_id=ws)
            other.import_archive(pcx.export("workspace", workspace_id=ws).data, commit=True, skip_existing=True)
            other.update_card("mine\nmore", expected_version=1, workspace_id=ws)
            time.sleep(1.1)
            pcx.update_card("theirs, later", expected_version=1, workspace_id=ws)
            cards = pcx.export("workspace", workspace_id=ws).data
            kept = other.import_archive(cards, skip_existing=True)
            card = next(c for c in kept.conflicts if c.kind == "card")
            assert card.resolution == "keep" and card.target is not None and card.archive is not None
            assert (card.target.version, card.target.first_line) == (2, "mine")
            assert (card.archive.version, card.archive.first_line) == (2, "theirs, later")
            assert card.archive.updated_at > card.target.updated_at
            taken = other.import_archive(cards, commit=True, skip_existing=True, cards_newer=True)
            assert [c.resolution for c in taken.conflicts if c.kind == "card"] == ["update"]
            head, _ = other.get_card(workspace_id=ws)
            assert head is not None and head.text == "theirs, later" and head.version == 3
    finally:
        fresh.stop()
