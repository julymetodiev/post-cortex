# Copyright (c) 2026 Chronicle Labs Ltd.
# MIT License
"""Plain data types of the post-cortex client."""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Literal, Optional

from ._pb import pcx_pb2 as pb

RecordKind = Literal[
    "qa",
    "decision_made",
    "problem_solved",
    "code_change",
    "requirement_added",
    "concept_defined",
]
"""Kind of a record (``interaction_type`` on the wire)."""

EntityType = Literal[
    "technology", "concept", "problem", "solution", "decision", "code_component"
]
"""Type of an entity."""

RelationType = Literal[
    "depends_on",
    "implements",
    "caused_by",
    "solves",
    "leads_to",
    "related_to",
    "required_by",
    "conflicts_with",
]
"""Type of a relation between two entities."""

SearchMode = Literal["fast", "local", "accurate"]
"""Ranking effort: vector + BM25 only, plus a local reranker, plus Voyage."""

Sources = Literal["records", "messages", "all"]
"""What a search covers: structured records, captured messages or both."""


@dataclass
class Entity:
    """A named thing a record is about."""

    name: str
    type: EntityType = "concept"


@dataclass
class Relation:
    """A directed relation between two entities of the same record."""

    from_entity: str
    to_entity: str
    type: RelationType
    context: str
    """Why the relation holds (required, non-empty)."""


@dataclass
class CodeRef:
    """Where in the code a record points."""

    file_path: str
    start_line: int = 0
    end_line: int = 0
    code_snippet: str = ""
    commit_hash: str = ""
    branch: str = ""
    change_description: str = ""


@dataclass
class Record:
    """One record to write (see :meth:`Client.record`)."""

    kind: RecordKind
    title: str
    description: str = ""
    entities: list[Entity] = field(default_factory=list)
    relations: list[Relation] = field(default_factory=list)
    details: list[str] = field(default_factory=list)
    examples: list[str] = field(default_factory=list)
    implications: list[str] = field(default_factory=list)
    code_ref: Optional[CodeRef] = None
    derived_from: list[str] = field(default_factory=list)
    """Captured messages the record was drawn from (``message:<seq>``)."""
    valid_from: Optional[str] = None
    """When the fact started to hold, if not now (RFC 3339 or ``YYYY-MM-DD``)."""
    supersedes: list[str] = field(default_factory=list)
    """Entry ids of earlier records of the session this one replaces."""
    confidence: Optional[float] = None
    """How sure the writer is, 0..1."""


@dataclass
class Session:
    id: str
    name: str
    description: str
    created_at: datetime
    update_count: int


@dataclass
class Workspace:
    id: str
    name: str
    description: str
    created_at: datetime
    sessions: dict[str, str]
    """Session id -> role."""
    archived_at: Optional[datetime] = None
    """When the workspace was archived; ``None`` while it is active."""

    @property
    def archived(self) -> bool:
        return self.archived_at is not None


@dataclass
class Project:
    """The workspace and session of a project key (see :meth:`Client.resolve_project`)."""

    workspace_id: str
    session_id: str
    created: bool
    """True: this call bound the key (a new workspace, or the key added to one)."""


@dataclass
class Card:
    """A project card (``workspace:<id>``) or the user card (``user``)."""

    scope: str
    version: int
    text: str
    updated_at: datetime
    author: Optional[str] = None


@dataclass
class CardWrite:
    """Outcome of :meth:`Client.update_card`."""

    written: bool
    """False: the card changed since ``expected_version``; ``card`` is the latest."""
    card: Optional[Card]


@dataclass
class Message:
    """A captured conversation turn to append."""

    turn_id: str
    """The client's id for the turn; appends are idempotent on it."""
    role: Literal["user", "assistant"]
    text: str
    author: Optional[str] = None
    source_session: Optional[str] = None
    timestamp: Optional[datetime] = None
    tools: list[str] = field(default_factory=list)
    files: list[str] = field(default_factory=list)


@dataclass
class Assembled:
    """Context assembled for a prompt (see :meth:`Client.assemble`)."""

    formatted_text: str
    total_tokens: int
    items: list[pb.AssembledContextItem]
    entity_context: list[pb.AssembledEntityContext]
    impact: list[pb.ImpactEntry]
    cards: list[Card]


RadarKind = Literal[
    "unsolved_problem",
    "decision_without_rationale",
    "open_question",
    "stale_entity",
    "alias_candidate",
]
"""Radar list of a dismissed item. The key is the entity name (``unsolved_problem``,
``stale_entity``), the record id (``decision_without_rationale``), the question text
(``open_question``) or ``"<name>|<of>"`` (``alias_candidate``)."""


@dataclass
class RadarDismissal:
    """A radar item a workspace dismissed (see :meth:`Client.dismiss_radar_item`)."""

    kind: str
    key: str
    dismissed_at: datetime
    author: Optional[str] = None


@dataclass
class ExportSummary:
    """What an export holds (the last chunk of :meth:`Client.export`)."""

    format_version: str
    sessions: int
    workspaces: int
    updates: int
    checkpoints: int
    embeddings: int
    total_bytes: int
    compression: str
    messages: int = 0
    cards: int = 0
    """Card versions."""


@dataclass
class Archive:
    """An export: the bytes of a ``pcx export`` file and its summary."""

    data: bytes
    summary: ExportSummary


@dataclass
class ImportCardHead:
    """One side's latest version of a card whose history diverged."""

    version: int
    """In its own history (two histories' numbers are not comparable)."""
    updated_at: datetime
    first_line: str


@dataclass
class ImportConflict:
    """A session or workspace of the archive the daemon already has, or a card whose
    history diverged from the daemon's."""

    kind: str
    """``session``, ``workspace`` or ``card``."""
    id: str
    """A card's workspace; the nil uuid for the user card."""
    name: str
    resolution: str
    """``error``, ``skip`` or ``overwrite``; for a card also ``keep`` (the daemon's stays) or
    ``update`` (``cards_newer`` and the archive's was written later)."""
    target: Optional[ImportCardHead] = None
    """A card: the daemon's latest version."""
    archive: Optional[ImportCardHead] = None
    """A card: the archive's latest version."""


@dataclass
class ImportResult:
    """Outcome of :meth:`Client.import_archive`: a preview, or (``committed``) the import."""

    committed: bool
    format_version: str
    sessions: int
    workspaces: int
    updates: int
    checkpoints: int
    embeddings: int
    conflicts: list[ImportConflict]
    sessions_imported: int = 0
    sessions_skipped: int = 0
    workspaces_imported: int = 0
    workspaces_skipped: int = 0
    updates_imported: int = 0
    checkpoints_imported: int = 0
    embeddings_imported: int = 0
    errors: list[str] = field(default_factory=list)
    messages: int = 0
    messages_new: int = 0
    """Messages whose turn the daemon does not store yet."""
    cards: int = 0
    messages_imported: int = 0
    messages_skipped: int = 0
    cards_imported: int = 0
    cards_skipped: int = 0


def from_unix(seconds: int) -> datetime:
    return datetime.fromtimestamp(seconds, tz=timezone.utc)
