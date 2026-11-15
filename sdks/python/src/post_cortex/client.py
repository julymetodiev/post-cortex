# Copyright (c) 2026 Chronicle Labs Ltd.
# MIT License
"""gRPC client for the post-cortex daemon (``pcx.v1.PostCortex``)."""

from __future__ import annotations

import os
from datetime import datetime
from typing import Callable, Iterable, Iterator, Optional, Sequence, TypeVar, overload

import grpc

from ._pb import pcx_pb2 as pb
from ._pb import pcx_pb2_grpc as pb_grpc
from .types import (
    Archive,
    Assembled,
    Card,
    CardWrite,
    CodeRef,
    Entity,
    ExportSummary,
    ImportCardHead,
    ImportConflict,
    ImportResult,
    Message,
    Project,
    RadarDismissal,
    Record,
    RecordKind,
    Relation,
    SearchMode,
    Session,
    Sources,
    Workspace,
    from_unix,
)

_Req = TypeVar("_Req")
_Res = TypeVar("_Res")

DEFAULT_TARGET = "127.0.0.1:3738"
"""The daemon's default gRPC address."""

IMPORT_CHUNK_BYTES = 1 << 20
"""Archive bytes per chunk :meth:`Client.import_archive` sends."""


class PostCortexError(Exception):
    """A failed call; ``code`` is the gRPC status code."""

    def __init__(self, code: grpc.StatusCode, message: str):
        super().__init__(f"{code.name}: {message}")
        self.code = code
        self.message = message


class NotFound(PostCortexError):
    """The session, workspace, entity or record does not exist."""


class InvalidArgument(PostCortexError):
    """The request was rejected (missing entities, unknown relation type, ...)."""


class Unauthenticated(PostCortexError):
    """The daemon needs a bearer token (``token=`` or ``PC_AUTH_TOKEN``)."""


_ERRORS = {
    grpc.StatusCode.NOT_FOUND: NotFound,
    grpc.StatusCode.INVALID_ARGUMENT: InvalidArgument,
    grpc.StatusCode.UNAUTHENTICATED: Unauthenticated,
}


def _error(e: grpc.RpcError) -> PostCortexError:
    code = e.code() if hasattr(e, "code") else grpc.StatusCode.UNKNOWN
    details = e.details() if hasattr(e, "details") else str(e)
    return _ERRORS.get(code, PostCortexError)(code, details or "")


class Client:
    """A connection to a post-cortex daemon.

    >>> pcx = Client()                       # 127.0.0.1:3738, PC_AUTH_TOKEN, PC_AUTHOR
    >>> sid = pcx.create_session("my project")
    >>> pcx.record(sid, Record("decision_made", "Use SurrealDB", "Shared server, namespaces",
    ...     entities=[Entity("SurrealDB", "technology"), Entity("storage", "concept")],
    ...     relations=[Relation("storage", "SurrealDB", "depends_on", "chosen backend")]))
    >>> pcx.search("which database?", session_id=sid)

    ``author`` names who the records come from (an agent such as ``my-bot``, or a person);
    it defaults to ``PC_AUTHOR``. ``secure=True`` uses TLS (system roots) instead of a plain
    connection; ``timeout`` is per call, in seconds.
    """

    def __init__(
        self,
        target: Optional[str] = None,
        *,
        token: Optional[str] = None,
        author: Optional[str] = None,
        secure: bool = False,
        timeout: float = 30.0,
        channel: Optional[grpc.Channel] = None,
    ):
        self.target = target or os.environ.get("PC_GRPC_TARGET", DEFAULT_TARGET)
        self.token = token if token is not None else os.environ.get("PC_AUTH_TOKEN")
        self.author = author if author is not None else os.environ.get("PC_AUTHOR")
        self.timeout = timeout
        if channel is None:
            channel = (
                grpc.secure_channel(self.target, grpc.ssl_channel_credentials())
                if secure
                else grpc.insecure_channel(self.target)
            )
        self._channel = channel
        self.stub = pb_grpc.PostCortexStub(channel)
        """The generated stub, for RPCs the client does not wrap."""

    def close(self) -> None:
        self._channel.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    @overload
    def _call(self, rpc: grpc.UnaryUnaryMultiCallable[_Req, _Res], request: _Req) -> _Res: ...
    @overload
    def _call(
        self, rpc: grpc.UnaryStreamMultiCallable[_Req, _Res], request: _Req
    ) -> Iterator[_Res]: ...
    @overload
    def _call(
        self, rpc: grpc.StreamUnaryMultiCallable[_Req, _Res], request: Iterator[_Req]
    ) -> _Res: ...
    def _call(self, rpc: Callable[..., object], request: object) -> object:
        """One RPC of the stub, with the token and the timeout; its failure as a
        :class:`PostCortexError`."""
        metadata = [("authorization", f"Bearer {self.token}")] if self.token else None
        try:
            return rpc(request, timeout=self.timeout, metadata=metadata)
        except grpc.RpcError as e:
            raise _error(e) from None

    # ------------------------------------------------------------------ health

    def health(self) -> pb.HealthResponse:
        return self._call(self.stub.Health, pb.HealthRequest())

    # ---------------------------------------------------------------- sessions

    def create_session(self, name: str = "", description: str = "") -> str:
        """Create a session; returns its id."""
        return self._call(self.stub.CreateSession, pb.CreateSessionRequest(name=name, description=description)
        ).session_id

    def list_sessions(self, name_filter: str = "", limit: int = 0) -> list[Session]:
        r = self._call(self.stub.ListSessions, pb.ListSessionsRequest(name_filter=name_filter, limit=limit))
        return [_session(s) for s in r.sessions]

    def get_session(self, session_id: str) -> Session:
        return _session(self._call(self.stub.LoadSession, pb.LoadSessionRequest(session_id=session_id)).session)

    def search_sessions(self, query: str) -> list[Session]:
        """Sessions whose name or description contains ``query``."""
        r = self._call(self.stub.SearchSessions, pb.SearchSessionsRequest(query=query))
        return [_session(s) for s in r.sessions]

    def update_session(
        self, session_id: str, *, name: Optional[str] = None, description: Optional[str] = None
    ) -> bool:
        req = pb.UpdateSessionMetadataRequest(session_id=session_id)
        if name is not None:
            req.name = name
        if description is not None:
            req.description = description
        return self._call(self.stub.UpdateSessionMetadata, req).success

    def delete_session(self, session_id: str) -> bool:
        return self._call(self.stub.DeleteSession, pb.DeleteSessionRequest(session_id=session_id)).success

    # -------------------------------------------------------------- workspaces

    def create_workspace(self, name: str, description: str = "") -> str:
        return self._call(self.stub.CreateWorkspace, pb.CreateWorkspaceRequest(name=name, description=description)
        ).workspace_id

    def list_workspaces(self, include_archived: bool = False) -> list[Workspace]:
        """The workspaces; archived ones only with ``include_archived``."""
        r = self._call(self.stub.ListWorkspaces, pb.ListWorkspacesRequest(include_archived=include_archived))
        return [_workspace(w) for w in r.workspaces]

    def update_workspace(
        self, workspace_id: str, *, name: Optional[str] = None, description: Optional[str] = None
    ) -> Workspace:
        """Rename a workspace and/or change its description; returns it as it is now."""
        req = pb.UpdateWorkspaceRequest(workspace_id=workspace_id)
        if name is not None:
            req.name = name
        if description is not None:
            req.description = description
        return _workspace(self._call(self.stub.UpdateWorkspace, req).workspace)

    def archive_workspace(self, workspace_id: str) -> bool:
        """Archive a workspace (kept, left out of :meth:`list_workspaces`); returns whether
        its state changed."""
        req = pb.ArchiveWorkspaceRequest(workspace_id=workspace_id)
        return self._call(self.stub.ArchiveWorkspace, req).changed

    def unarchive_workspace(self, workspace_id: str) -> bool:
        req = pb.ArchiveWorkspaceRequest(workspace_id=workspace_id)
        return self._call(self.stub.UnarchiveWorkspace, req).changed

    def get_workspace(self, workspace_id: str) -> Workspace:
        r = self._call(self.stub.GetWorkspace, pb.GetWorkspaceRequest(workspace_id=workspace_id))
        return _workspace(r.workspace)

    def delete_workspace(self, workspace_id: str) -> bool:
        return self._call(self.stub.DeleteWorkspace, pb.DeleteWorkspaceRequest(workspace_id=workspace_id)
        ).success

    def add_session(self, workspace_id: str, session_id: str, role: str = "primary") -> bool:
        """Add a session to a workspace (``primary``, ``related``, ``dependency``, ``shared``)."""
        return self._call(self.stub.AddSessionToWorkspace,
            pb.AddSessionToWorkspaceRequest(workspace_id=workspace_id, session_id=session_id, role=role),
        ).success

    def remove_session(self, workspace_id: str, session_id: str) -> bool:
        return self._call(self.stub.RemoveSessionFromWorkspace,
            pb.RemoveSessionFromWorkspaceRequest(workspace_id=workspace_id, session_id=session_id),
        ).success

    def resolve_project(
        self,
        key: str,
        name: str,
        client: Optional[str] = None,
        workspace: Optional[str] = None,
    ) -> Project:
        """The workspace and session of project ``key``, created on first use.

        ``key`` is a normalised git remote (``github.com/acme/api``) or ``local:<hash>``; the
        same key always gets the same workspace and session. A new key gets a session named
        ``name`` in the workspace named ``workspace`` (created if missing), else in a new
        workspace named ``name``. ``client`` names the caller in the daemon's logs.
        """
        req = pb.ResolveProjectRequest(key=key, name=name)
        if client is not None:
            req.client = client
        if workspace is not None:
            req.workspace = workspace
        r = self._call(self.stub.ResolveProject, req)
        return Project(r.workspace_id, r.session_id, r.created)

    # ----------------------------------------------------------------- records

    def record(self, session_id: str, record: Record, *, author: Optional[str] = None) -> str:
        """Store one record; returns its entry id.

        Every record needs at least one entity and one relation, both ends of each relation
        among its entities. The write returns once it is stored; embedding and graph merge
        follow on the daemon in the background.
        """
        req = pb.UpdateContextRequest(
            session_id=session_id,
            interaction_type=record.kind,
            content=_content(record),
            entities=[_entity(e) for e in record.entities],
            relations=[_relation(r) for r in record.relations],
            derived_from=list(record.derived_from),
            supersedes=list(record.supersedes),
        )
        who = author or self.author
        if who:
            req.author = who
        if record.valid_from:
            req.valid_from = record.valid_from
        if record.confidence is not None:
            req.confidence = record.confidence
        r = self._call(self.stub.UpdateContext, req)
        return r.update_id

    def record_many(
        self, session_id: str, records: Sequence[Record], *, author: Optional[str] = None
    ) -> list[str]:
        """Store several records in one call; returns their entry ids in order."""
        items = []
        for record in records:
            item = pb.ContextUpdateItem(
                interaction_type=record.kind,
                content=_content(record),
                entities=[_entity(e) for e in record.entities],
                relations=[_relation(r) for r in record.relations],
                derived_from=list(record.derived_from),
                supersedes=list(record.supersedes),
            )
            if record.valid_from:
                item.valid_from = record.valid_from
            if record.confidence is not None:
                item.confidence = record.confidence
            items.append(item)
        req = pb.BulkUpdateContextRequest(session_id=session_id, updates=items)
        who = author or self.author
        if who:
            req.author = who
        r = self._call(self.stub.BulkUpdateContext, req)
        if r.failure_count:
            raise InvalidArgument(
                grpc.StatusCode.INVALID_ARGUMENT,
                f"{r.failure_count} of {len(records)} records were rejected",
            )
        return list(r.update_ids)

    def records(
        self,
        session_id: str,
        *,
        kind: Optional[RecordKind] = None,
        limit: int = 50,
        after: Optional[datetime] = None,
    ) -> list[pb.ContextUpdateEntry]:
        """The session's records, oldest first, optionally of one kind and after a time."""
        req = pb.QueryContextRequest(
            session_id=session_id,
            interaction_type=kind or "",
            limit=limit,
            after_unix=int(after.timestamp()) if after else 0,
        )
        return list(self._call(self.stub.QueryContext, req).updates)

    # ------------------------------------------------------------------ search

    def search(
        self,
        query: str,
        *,
        session_id: Optional[str] = None,
        workspace_id: Optional[str] = None,
        limit: int = 10,
        mode: Optional[SearchMode] = None,
        sources: Optional[Sources] = None,
        author: Optional[str] = None,
        prefer_author: Optional[str] = None,
        min_score: float = 0.0,
        trace: bool = False,
    ) -> list[pb.SearchResult]:
        """Hybrid search over a session, a workspace, or everything (neither given).

        Hits come with their neighbouring records (``neighbour_of`` set). Records that no
        longer hold carry ``validity`` (its ``valid_to`` and ``reason``); ``trace=True``
        fills ``signals``.
        """
        req = pb.SemanticSearchRequest(
            query=query, max_results=limit, session_id=session_id or "", min_score=min_score
        )
        if workspace_id:
            req.workspace_id = workspace_id
        if mode:
            req.mode = mode
        if sources:
            req.sources = sources
        if author:
            req.author = author
        if prefer_author:
            req.prefer_author = prefer_author
        if trace:
            req.trace = True
        r = self._call(self.stub.SemanticSearch, req)
        return list(r.results)

    def assemble(
        self,
        query: str,
        *,
        session_id: Optional[str] = None,
        workspace_id: Optional[str] = None,
        token_budget: int = 4000,
        sources: Optional[Sources] = None,
    ) -> Assembled:
        """Context for a prompt within ``token_budget``: cards, the entities the query names,
        their graph neighbourhood and dependents, records and (with ``sources``) excerpts."""
        req = pb.AssembleContextRequest(
            session_id=session_id or "", query=query, token_budget=token_budget
        )
        if workspace_id:
            req.workspace_id = workspace_id
        if sources:
            req.sources = sources
        r = self._call(self.stub.AssembleContext, req)
        return Assembled(
            formatted_text=r.formatted_text,
            total_tokens=r.total_tokens,
            items=list(r.items),
            entity_context=list(r.entity_context),
            impact=list(r.impact),
            cards=[_card(c) for c in r.cards],
        )

    # ------------------------------------------------------------- graph views

    def _view(self, req: pb.GetGraphViewRequest) -> pb.GraphView:
        return self._call(self.stub.GetGraphView, req)

    @staticmethod
    def _scope(session_id: Optional[str], workspace_id: Optional[str]) -> pb.GetGraphViewRequest:
        """A graph view request of a session, or of a workspace."""
        return pb.GetGraphViewRequest(session_id=session_id or "", workspace_id=workspace_id or "")

    def entity(
        self,
        name: str,
        *,
        limit: int = 100,
        session_id: Optional[str] = None,
        workspace_id: Optional[str] = None,
    ) -> pb.EntityView:
        """An entity by name or alias: relations, records, decisions."""
        params = pb.EntityViewParams(entity=name, limit=limit)
        req = self._scope(session_id, workspace_id)
        req.entity.CopyFrom(params)
        return self._view(req).entity

    def impact(
        self,
        name: str,
        *,
        depth: int = 3,
        limit: int = 100,
        session_id: Optional[str] = None,
        workspace_id: Optional[str] = None,
    ) -> pb.ImpactView:
        """What depends on an entity, and the valid decisions, requirements, problems."""
        params = pb.ImpactViewParams(entity=name, depth=depth, limit=limit)
        req = self._scope(session_id, workspace_id)
        req.impact.CopyFrom(params)
        return self._view(req).impact

    def lineage(
        self,
        name: str,
        *,
        depth: int = 3,
        session_id: Optional[str] = None,
        workspace_id: Optional[str] = None,
    ) -> pb.LineageView:
        """Problem -> decision -> consequences around an entity, in time order."""
        params = pb.LineageViewParams(entity=name, depth=depth)
        req = self._scope(session_id, workspace_id)
        req.lineage.CopyFrom(params)
        return self._view(req).lineage

    def topics(
        self,
        *,
        limit: int = 100,
        per_topic: int = 5,
        session_id: Optional[str] = None,
        workspace_id: Optional[str] = None,
    ) -> pb.TopicsView:
        """Entity communities with their central entities and key decisions."""
        params = pb.TopicsViewParams(limit=limit, per_topic=per_topic)
        req = self._scope(session_id, workspace_id)
        req.topics.CopyFrom(params)
        return self._view(req).topics

    def radar(
        self,
        *,
        limit: int = 100,
        include_dismissed: bool = False,
        session_id: Optional[str] = None,
        workspace_id: Optional[str] = None,
    ) -> pb.RadarView:
        """Unsolved problems, decisions without rationale, open questions, stale entities.
        Items the workspace dismissed are left out (and listed under ``dismissed``) unless
        ``include_dismissed``."""
        params = pb.RadarViewParams(limit=limit, include_dismissed=include_dismissed)
        req = self._scope(session_id, workspace_id)
        req.radar.CopyFrom(params)
        return self._view(req).radar

    def dismiss_radar_item(
        self, workspace_id: str, kind: str, key: str, *, author: Optional[str] = None
    ) -> RadarDismissal:
        """Hide a radar item from the radar of the workspace and of its sessions. ``kind``
        is a :data:`~post_cortex.types.RadarKind`."""
        req = pb.DismissRadarItemRequest(workspace_id=workspace_id, kind=kind, key=key)
        author = author if author is not None else self.author
        if author:
            req.author = author
        return _dismissal(self._call(self.stub.DismissRadarItem, req).dismissal)

    def restore_radar_item(self, workspace_id: str, kind: str, key: str) -> bool:
        """Show a dismissed radar item again; returns whether it was dismissed."""
        req = pb.RestoreRadarItemRequest(workspace_id=workspace_id, kind=kind, key=key)
        return self._call(self.stub.RestoreRadarItem, req).existed

    def list_radar_dismissals(self, workspace_id: str) -> list[RadarDismissal]:
        req = pb.ListRadarDismissalsRequest(workspace_id=workspace_id)
        return [_dismissal(d) for d in self._call(self.stub.ListRadarDismissals, req).dismissals]

    def file_context(
        self,
        path: str,
        *,
        depth: int = 2,
        limit: int = 20,
        session_id: Optional[str] = None,
        workspace_id: Optional[str] = None,
    ) -> pb.FileView:
        """Past problems, decisions and dependents of a source file."""
        params = pb.FileViewParams(path=path, depth=depth, limit=limit)
        req = self._scope(session_id, workspace_id)
        req.file.CopyFrom(params)
        return self._view(req).file

    # -------------------------------------------------------- entity and record upkeep

    def delete_entity(self, session_id: str, name: str) -> bool:
        return self._call(self.stub.DeleteEntity, pb.DeleteEntityRequest(session_id=session_id, entity_name=name)
        ).existed

    def merge_entities(self, session_id: str, name: str, into: str) -> bool:
        """Make ``name`` an alias of ``into``; mentions, relations and counts move over."""
        return self._call(self.stub.MergeEntities, pb.MergeEntitiesRequest(session_id=session_id, entity_name=name, into=into)
        ).merged

    def forget(self, session_id: str, entry_id: str) -> bool:
        """The record stops holding now; it stays in the history."""
        return self._call(self.stub.ForgetUpdate, pb.UpdateRefRequest(session_id=session_id, entry_id=entry_id)
        ).existed

    def purge(self, session_id: str, entry_id: str) -> bool:
        """Erase the record and everything derived from it. Cannot be undone."""
        return self._call(self.stub.PurgeUpdate, pb.UpdateRefRequest(session_id=session_id, entry_id=entry_id)
        ).existed

    # ------------------------------------------------------------------- cards

    def get_card(
        self, *, workspace_id: Optional[str] = None, history: int = 0
    ) -> tuple[Optional[Card], list[Card]]:
        """The project card of ``workspace_id``, or the user card; with older versions."""
        req = pb.GetCardRequest(history=history)
        if workspace_id:
            req.workspace_id = workspace_id
        else:
            req.user = True
        r = self._call(self.stub.GetCard, req)
        return (_card(r.card) if r.HasField("card") else None, [_card(c) for c in r.history])

    def update_card(
        self,
        content: str,
        *,
        expected_version: int,
        workspace_id: Optional[str] = None,
        author: Optional[str] = None,
    ) -> CardWrite:
        """Write the next version (at most 2 KB). ``expected_version`` is the version the text
        was edited from (0 for a new card); if the card changed since, nothing is written and
        the latest version comes back to merge with."""
        req = pb.UpdateCardRequest(content=content, expected_version=expected_version)
        if workspace_id:
            req.workspace_id = workspace_id
        else:
            req.user = True
        who = author or self.author
        if who:
            req.author = who
        r = self._call(self.stub.UpdateCard, req)
        return CardWrite(r.written, _card(r.card) if r.HasField("card") else None)

    # ---------------------------------------------------------------- messages

    def append_messages(
        self, session_id: str, messages: Iterable[Message]
    ) -> pb.AppendMessagesResponse:
        """Append captured conversation turns; idempotent on each ``turn_id``."""
        wire = []
        for m in messages:
            msg = pb.CapturedMessage(
                turn_id=m.turn_id, role=m.role, text=m.text, tools=list(m.tools), files=list(m.files)
            )
            who = m.author or self.author
            if who:
                msg.author = who
            if m.source_session:
                msg.source_session = m.source_session
            if m.timestamp:
                msg.timestamp_unix_ms = int(m.timestamp.timestamp() * 1000)
            wire.append(msg)
        req = pb.AppendMessagesRequest(session_id=session_id, messages=wire)
        return self._call(self.stub.AppendMessages, req)

    # --------------------------------------------------------------- summaries

    def delete_message(self, session_id: str, seq: int) -> pb.DeleteMessagesResponse:
        """Delete the captured turn at ``seq`` (``message:<seq>``). Records drawn from it
        keep the id. Returns ``deleted`` (0 or 1) and ``remaining``."""
        return self._call(self.stub.DeleteMessage, pb.DeleteMessageRequest(session_id=session_id, seq=seq))

    def delete_messages(self, session_id: str) -> pb.DeleteMessagesResponse:
        """Delete the session's whole captured message stream; its records stay."""
        return self._call(self.stub.DeleteMessages, pb.DeleteMessagesRequest(session_id=session_id))

    # ---------------------------------------------------------- export / import

    def export_chunks(
        self,
        scope: str = "all",
        *,
        session_ids: Optional[Sequence[str]] = None,
        workspace_id: Optional[str] = None,
        include_checkpoints: bool = False,
        compression: str = "none",
        no_messages: bool = False,
    ) -> Iterator[pb.ExportChunk]:
        """The export as it streams: ``ExportChunk`` messages (``data``; the last one has
        ``last`` and ``summary``). See :meth:`export`."""
        req = pb.ExportRequest(
            scope=scope,
            session_ids=list(session_ids or []),
            workspace_id=workspace_id or "",
            include_checkpoints=include_checkpoints,
            compression=compression,
            no_messages=no_messages,
        )
        stream = self._call(self.stub.Export, req)
        try:
            yield from stream
        except grpc.RpcError as e:
            raise _error(e) from None

    def export(
        self,
        scope: str = "all",
        *,
        session_ids: Optional[Sequence[str]] = None,
        workspace_id: Optional[str] = None,
        include_checkpoints: bool = False,
        compression: str = "none",
        no_messages: bool = False,
    ) -> Archive:
        """Export ``scope`` (``all``, ``session`` with ``session_ids``, ``workspace`` with
        ``workspace_id``): the bytes are a ``pcx export`` file (``compression`` ``none``,
        ``gzip`` or ``zstd``), readable by ``pcx import`` and :meth:`import_archive`.
        ``no_messages`` leaves the captured message streams out (cards stay)."""
        data = bytearray()
        summary = None
        for chunk in self.export_chunks(
            scope,
            session_ids=session_ids,
            workspace_id=workspace_id,
            include_checkpoints=include_checkpoints,
            compression=compression,
            no_messages=no_messages,
        ):
            data += chunk.data
            if chunk.last and chunk.HasField("summary"):
                s = chunk.summary
                summary = ExportSummary(
                    s.format_version,
                    s.sessions,
                    s.workspaces,
                    s.updates,
                    s.checkpoints,
                    s.embeddings,
                    s.total_bytes,
                    s.compression,
                    s.messages,
                    s.cards,
                )
        if summary is None:
            raise PostCortexError(grpc.StatusCode.DATA_LOSS, "export ended without a summary")
        return Archive(bytes(data), summary)

    def import_archive(
        self,
        data: bytes,
        *,
        commit: bool = False,
        skip_existing: bool = False,
        overwrite: bool = False,
        session_ids: Optional[Sequence[str]] = None,
        workspace_ids: Optional[Sequence[str]] = None,
        chunk_size: int = IMPORT_CHUNK_BYTES,
        cards_newer: bool = False,
    ) -> ImportResult:
        """Send a ``pcx export`` file (JSON, gzip or zstd). Without ``commit`` nothing is
        written: the result is a preview (counts, conflicts). ``skip_existing`` leaves the
        sessions and workspaces the daemon has, ``overwrite`` replaces them; with neither they
        are reported as errors. ``session_ids`` / ``workspace_ids`` limit what is imported.
        A card whose history diverged keeps the daemon's latest version unless ``overwrite``
        (the archive's history replaces it) or ``cards_newer`` (the archive's latest version,
        when written later, is added as the next one)."""
        if chunk_size <= 0:
            raise ValueError("chunk_size must be positive")
        settings = pb.ImportSettings(
            commit=commit,
            skip_existing=skip_existing,
            overwrite=overwrite,
            session_ids=list(session_ids or []),
            workspace_ids=list(workspace_ids or []),
            cards_newer=cards_newer,
        )

        def chunks() -> Iterator[pb.ImportChunk]:
            view = memoryview(data)
            yield pb.ImportChunk(data=bytes(view[:chunk_size]), settings=settings)
            for start in range(chunk_size, len(view), chunk_size):
                yield pb.ImportChunk(data=bytes(view[start : start + chunk_size]))

        r = self._call(self.stub.Import, chunks())
        return ImportResult(
            committed=r.committed,
            format_version=r.format_version,
            sessions=r.sessions,
            workspaces=r.workspaces,
            updates=r.updates,
            checkpoints=r.checkpoints,
            embeddings=r.embeddings,
            conflicts=[
                ImportConflict(
                    c.kind,
                    c.id,
                    c.name,
                    c.resolution,
                    _card_head(c.target) if c.HasField("target") else None,
                    _card_head(c.archive) if c.HasField("archive") else None,
                )
                for c in r.conflicts
            ],
            sessions_imported=r.sessions_imported,
            sessions_skipped=r.sessions_skipped,
            workspaces_imported=r.workspaces_imported,
            workspaces_skipped=r.workspaces_skipped,
            updates_imported=r.updates_imported,
            checkpoints_imported=r.checkpoints_imported,
            embeddings_imported=r.embeddings_imported,
            errors=list(r.errors),
            messages=r.messages,
            messages_new=r.messages_new,
            cards=r.cards,
            messages_imported=r.messages_imported,
            messages_skipped=r.messages_skipped,
            cards_imported=r.cards_imported,
            cards_skipped=r.cards_skipped,
        )

    def structured_summary(
        self,
        session_id: str,
        *,
        compact: bool = True,
        decisions_limit: Optional[int] = None,
        entities_limit: Optional[int] = None,
        questions_limit: Optional[int] = None,
        concepts_limit: Optional[int] = None,
        insights_limit: Optional[int] = None,
        min_confidence: Optional[float] = None,
    ) -> pb.StructuredSummary:
        """Decisions, insights, entities and questions of a session."""
        req = pb.GetStructuredSummaryRequest(
            session_id=session_id,
            compact=compact,
            decisions_limit=decisions_limit,
            entities_limit=entities_limit,
            questions_limit=questions_limit,
            concepts_limit=concepts_limit,
            insights_limit=insights_limit,
            min_confidence=min_confidence,
        )
        return self._call(self.stub.GetStructuredSummary, req)

    def key_decisions(self, session_id: str) -> list[pb.DecisionSummary]:
        """The session's key decisions."""
        r = self._call(self.stub.GetKeyDecisions, pb.GetKeyDecisionsRequest(session_id=session_id))
        return list(r.decisions)

    def session_statistics(self, session_id: str) -> pb.SessionStatistics:
        """The session's counts."""
        req = pb.GetSessionStatisticsRequest(session_id=session_id)
        return self._call(self.stub.GetSessionStatistics, req)

    # ------------------------------------------------------------------- admin

    def vectorize_session(self, session_id: str) -> int:
        """Embed what a session has not embedded yet; returns the vectors created."""
        return self._call(self.stub.VectorizeSession, pb.VectorizeSessionRequest(session_id=session_id)
        ).vectors_created

    def create_checkpoint(self, session_id: str) -> str:
        return self._call(self.stub.CreateCheckpoint, pb.CreateCheckpointRequest(session_id=session_id)
        ).checkpoint_id


# ---------------------------------------------------------------------- mapping


def _card_head(h: pb.ImportCardHead) -> ImportCardHead:
    return ImportCardHead(h.version, from_unix(h.updated_at_unix), h.first_line)


def _session(s: pb.SessionInfo) -> Session:
    return Session(s.session_id, s.name, s.description, from_unix(s.created_at_unix), s.update_count)


def _workspace(w: pb.WorkspaceInfo) -> Workspace:
    return Workspace(
        w.workspace_id,
        w.name,
        w.description,
        from_unix(w.created_at_unix),
        {e.session_id: e.role.lower() for e in w.sessions},
        from_unix(w.archived_at_unix) if w.archived else None,
    )


def _dismissal(d: pb.RadarDismissal) -> RadarDismissal:
    return RadarDismissal(
        d.kind,
        d.key,
        from_unix(d.dismissed_at_unix),
        d.author if d.HasField("author") else None,
    )


def _content(r: Record) -> pb.ContextContent:
    content = pb.ContextContent(
        title=r.title,
        description=r.description,
        details=list(r.details),
        examples=list(r.examples),
        implications=list(r.implications),
    )
    if r.code_ref:
        c: CodeRef = r.code_ref
        content.code_ref.CopyFrom(
            pb.CodeReference(
                file_path=c.file_path,
                start_line=c.start_line,
                end_line=c.end_line,
                code_snippet=c.code_snippet,
                commit_hash=c.commit_hash,
                branch=c.branch,
                change_description=c.change_description,
            )
        )
    return content


def _entity(e: Entity) -> pb.EntityInfo:
    return pb.EntityInfo(name=e.name, entity_type=e.type)


def _relation(r: Relation) -> pb.RelationInfo:
    return pb.RelationInfo(
        from_entity=r.from_entity, to_entity=r.to_entity, relation_type=r.type, context=r.context
    )


def _card(c: pb.CardInfo) -> Card:
    return Card(
        scope=c.scope,
        version=c.version,
        text=c.text,
        updated_at=from_unix(c.updated_at_unix),
        author=c.author if c.HasField("author") else None,
    )
