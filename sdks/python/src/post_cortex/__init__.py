# Copyright (c) 2026 Chronicle Labs Ltd.
# MIT License
"""Python client for post-cortex, long-term memory for AI agents.

Talks to the daemon's gRPC API (``pcx.v1.PostCortex``, default ``127.0.0.1:3738``).
"""

# The generated messages the client returns (pcx.v1, pcx.views.v1, pcx.blocks.v1).
from ._pb import pcx_blocks_pb2 as blocks
from ._pb import pcx_pb2 as proto
from ._pb import views_pb2 as views
from .client import (
    DEFAULT_TARGET,
    IMPORT_CHUNK_BYTES,
    Client,
    InvalidArgument,
    NotFound,
    PostCortexError,
    Unauthenticated,
)
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
    RadarKind,
    Record,
    Relation,
    Session,
    Workspace,
)

__version__ = "0.1.0"

__all__ = [
    "DEFAULT_TARGET",
    "IMPORT_CHUNK_BYTES",
    "Archive",
    "Assembled",
    "Card",
    "CardWrite",
    "Client",
    "CodeRef",
    "Entity",
    "ExportSummary",
    "ImportCardHead",
    "ImportConflict",
    "ImportResult",
    "InvalidArgument",
    "Message",
    "NotFound",
    "PostCortexError",
    "Project",
    "RadarDismissal",
    "RadarKind",
    "Record",
    "Relation",
    "Session",
    "Unauthenticated",
    "Workspace",
    "blocks",
    "proto",
    "views",
]
