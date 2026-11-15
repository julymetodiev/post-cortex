// Copyright (c) 2026 Chronicle Labs Ltd.
// MIT License

/**
 * Node.js client for post-cortex, long-term memory for AI agents.
 *
 * Talks to the daemon's gRPC API (`pcx.v1.PostCortex`, default `127.0.0.1:3738`).
 *
 * ```ts
 * import { Client } from "post-cortex";
 * const pcx = new Client();                  // PC_GRPC_TARGET, PC_AUTH_TOKEN, PC_AUTHOR
 * const sid = await pcx.createSession("my project");
 * await pcx.record(sid, {
 *   kind: "decision_made", title: "Use SurrealDB", description: "Shared server, namespaces",
 *   entities: [{ name: "SurrealDB", type: "technology" }, { name: "storage" }],
 *   relations: [{ from: "storage", to: "SurrealDB", type: "depends_on", context: "chosen backend" }],
 * });
 * const hits = await pcx.search("which database?", { sessionId: sid });
 * ```
 */

import { ChannelCredentials, credentials, Metadata, status, type ServiceError } from "@grpc/grpc-js";

import * as pb from "./gen/pcx.js";

export * as proto from "./gen/pcx.js";
export * as views from "./gen/views.js";
export * as blocks from "./gen/pcx_blocks.js";

/** The daemon's default gRPC address. */
export const DEFAULT_TARGET = "127.0.0.1:3738";


export const RECORD_KINDS = [
  "qa",
  "decision_made",
  "problem_solved",
  "code_change",
  "requirement_added",
  "concept_defined",
] as const;
export type RecordKind = (typeof RECORD_KINDS)[number];
export type EntityType =
  | "technology"
  | "concept"
  | "problem"
  | "solution"
  | "decision"
  | "code_component";
export type RelationType =
  | "depends_on"
  | "implements"
  | "caused_by"
  | "solves"
  | "leads_to"
  | "related_to"
  | "required_by"
  | "conflicts_with";
export type SearchMode = "fast" | "local" | "accurate";
export type Sources = "records" | "messages" | "all";

export interface Entity {
  name: string;
  /** Default `concept`. */
  type?: EntityType;
}

export interface Relation {
  from: string;
  to: string;
  type: RelationType;
  /** Why the relation holds (required, non-empty). */
  context: string;
}

export interface CodeRef {
  filePath: string;
  startLine?: number;
  endLine?: number;
  codeSnippet?: string;
  commitHash?: string;
  branch?: string;
  changeDescription?: string;
}

/** One record to write. Needs at least one entity and one relation. */
export interface RecordInput {
  kind: RecordKind;
  title: string;
  description?: string;
  entities: Entity[];
  relations: Relation[];
  details?: string[];
  examples?: string[];
  implications?: string[];
  codeRef?: CodeRef;
  /** Captured messages the record was drawn from (`message:<seq>`). */
  derivedFrom?: string[];
  /** When the fact started to hold, if not now (RFC 3339 or `YYYY-MM-DD`). */
  validFrom?: string;
  /** Entry ids of earlier records of the session this one replaces. */
  supersedes?: string[];
  /** How sure the writer is, 0..1. */
  confidence?: number;
}

export interface Session {
  id: string;
  name: string;
  description: string;
  createdAt: Date;
  updateCount: number;
}

export interface Workspace {
  id: string;
  name: string;
  description: string;
  createdAt: Date;
  /** Session id -> role. */
  sessions: Record<string, string>;
  /** When the workspace was archived; unset while it is active. */
  archivedAt?: Date;
}

/**
 * Radar list of a dismissed item. The key is the entity name (`unsolved_problem`,
 * `stale_entity`), the record id (`decision_without_rationale`), the question text
 * (`open_question`) or `"<name>|<of>"` (`alias_candidate`).
 */
export type RadarKind =
  | "unsolved_problem"
  | "decision_without_rationale"
  | "open_question"
  | "stale_entity"
  | "alias_candidate";

/** A radar item a workspace dismissed (see {@link Client.dismissRadarItem}). */
export interface RadarDismissal {
  kind: RadarKind;
  key: string;
  dismissedAt: Date;
  author?: string;
}

/** Outcome of {@link Client.deleteMessage} and {@link Client.deleteMessages}. */
export interface MessageDeletion {
  deleted: number;
  /** Turns left in the stream. */
  remaining: number;
}

export interface ExportOptions {
  /** `all` (default), `session` (with `sessionIds`) or `workspace` (with `workspaceId`). */
  scope?: "all" | "session" | "workspace";
  sessionIds?: string[];
  workspaceId?: string;
  includeCheckpoints?: boolean;
  /** `none` (default), `gzip` or `zstd`. */
  compression?: "none" | "gzip" | "zstd";
  /** Leave the captured message streams out (cards stay). */
  noMessages?: boolean;
}

export interface ExportSummary {
  formatVersion: string;
  sessions: number;
  workspaces: number;
  updates: number;
  checkpoints: number;
  embeddings: number;
  totalBytes: number;
  compression: string;
  messages: number;
  /** Card versions. */
  cards: number;
}

/** An export: the bytes of a `pcx export` file and its summary. */
export interface Archive {
  data: Buffer;
  summary: ExportSummary;
}

export interface ImportOptions {
  /** Write the archive; without it the result is a preview and nothing is written. */
  commit?: boolean;
  /** Leave the sessions and workspaces the daemon has. */
  skipExisting?: boolean;
  /** Replace them. With neither, they are reported as errors. */
  overwrite?: boolean;
  /**
   * A card whose history diverged: take the archive's latest version when it was written
   * later (added as the next version). Without it (and `overwrite`) the daemon's stays.
   */
  cardsNewer?: boolean;
  /** Only these sessions / workspaces. */
  sessionIds?: string[];
  workspaceIds?: string[];
  /** Bytes per streamed chunk (default {@link IMPORT_CHUNK_BYTES}). */
  chunkSize?: number;
}

export interface ImportConflict {
  kind: "session" | "workspace" | "card";
  /** A card's workspace; the nil uuid for the user card. */
  id: string;
  name: string;
  /**
   * For a card also `keep` (the daemon's stays) or `update` (`cardsNewer` and the archive's
   * was written later: added as the next version).
   */
  resolution: "error" | "skip" | "overwrite" | "keep" | "update";
  /** A card: the daemon's latest version. */
  target?: ImportCardHead;
  /** A card: the archive's latest version. */
  archive?: ImportCardHead;
}

/** One side's latest version of a card whose history diverged. */
export interface ImportCardHead {
  /** In its own history (two histories' numbers are not comparable). */
  version: number;
  updatedAt: Date;
  firstLine: string;
}

/** A preview of an import or, with `committed`, its outcome. */
export interface ImportResult {
  committed: boolean;
  formatVersion: string;
  sessions: number;
  workspaces: number;
  updates: number;
  checkpoints: number;
  embeddings: number;
  conflicts: ImportConflict[];
  sessionsImported: number;
  sessionsSkipped: number;
  workspacesImported: number;
  workspacesSkipped: number;
  updatesImported: number;
  checkpointsImported: number;
  embeddingsImported: number;
  errors: string[];
  messages: number;
  /** Messages whose turn the daemon does not store yet. */
  messagesNew: number;
  cards: number;
  messagesImported: number;
  messagesSkipped: number;
  cardsImported: number;
  cardsSkipped: number;
}

/** Archive bytes per chunk {@link Client.importArchive} sends. */
export const IMPORT_CHUNK_BYTES = 1 << 20;

/** The workspace and session of a project key (see {@link Client.resolveProject}). */
export interface Project {
  workspaceId: string;
  sessionId: string;
  /** True: this call bound the key (a new workspace, or the key added to one). */
  created: boolean;
}

export interface Card {
  /** `workspace:<id>` or `user`. */
  scope: string;
  version: number;
  text: string;
  updatedAt: Date;
  author?: string;
}

export interface CardWrite {
  /** False: the card changed since `expectedVersion`; `card` is the latest. */
  written: boolean;
  card?: Card;
}

export interface MessageInput {
  /** The client's id for the turn; appends are idempotent on it. */
  turnId: string;
  role: "user" | "assistant";
  text: string;
  author?: string;
  sourceSession?: string;
  timestamp?: Date;
  tools?: string[];
  files?: string[];
}

export interface Scope {
  sessionId?: string;
  workspaceId?: string;
}

export interface SearchOptions extends Scope {
  limit?: number;
  mode?: SearchMode;
  sources?: Sources;
  author?: string;
  preferAuthor?: string;
  minScore?: number;
  trace?: boolean;
}

export interface ClientOptions {
  /** `host:port`; default `PC_GRPC_TARGET` or {@link DEFAULT_TARGET}. */
  target?: string;
  /** Bearer token; default `PC_AUTH_TOKEN`. */
  token?: string;
  /** Who the records come from; default `PC_AUTHOR`. */
  author?: string;
  /** Use TLS (system roots). */
  secure?: boolean;
  /** Per-call deadline in milliseconds (default 30 000). */
  timeoutMs?: number;
  /** Custom channel credentials (overrides `secure`). */
  credentials?: ChannelCredentials;
}

/** A failed call; `code` is the gRPC status code. */
export class PostCortexError extends Error {
  constructor(
    readonly code: status,
    readonly details: string,
  ) {
    super(`${status[code] ?? code}: ${details}`);
    this.name = "PostCortexError";
  }
}
/** The session, workspace, entity or record does not exist. */
export class NotFound extends PostCortexError {}
/** The request was rejected (missing entities, unknown relation type, ...). */
export class InvalidArgument extends PostCortexError {}
/** The daemon needs a bearer token (`token` or `PC_AUTH_TOKEN`). */
export class Unauthenticated extends PostCortexError {}

function toError(e: ServiceError): PostCortexError {
  const Kind =
    e.code === status.NOT_FOUND
      ? NotFound
      : e.code === status.INVALID_ARGUMENT
        ? InvalidArgument
        : e.code === status.UNAUTHENTICATED
          ? Unauthenticated
          : PostCortexError;
  return new Kind(e.code, e.details ?? e.message);
}

/** A record's kind as the daemon lists it (its wire name). */
function recordKind(name: string): RecordKind {
  const kind = RECORD_KINDS.find((k) => k === name);
  if (kind === undefined) {
    throw new PostCortexError(status.INTERNAL, `unknown record kind ${name}`);
  }
  return kind;
}

/** A graph view answer without the view asked for: the daemon broke the contract. */
function missingView(view: string): never {
  throw new PostCortexError(status.INTERNAL, `GetGraphView answered without the ${view} view`);
}

const fromUnix = (seconds: number): Date => new Date(seconds * 1000);

const cardHead = (h: pb.ImportCardHead): ImportCardHead => ({
  version: h.version,
  updatedAt: fromUnix(h.updatedAtUnix),
  firstLine: h.firstLine,
});


type Unary<Req, Res> = (
  request: Req,
  metadata: Metadata,
  options: { deadline: Date },
  callback: (error: ServiceError | null, response: Res) => void,
) => unknown;

/** A connection to a post-cortex daemon. */
export class Client {
  readonly target: string;
  readonly author?: string;
  /** The generated stub, for RPCs this client does not wrap. */
  readonly stub: pb.PostCortexClient;
  private readonly token?: string;
  private readonly timeoutMs: number;

  constructor(options: ClientOptions = {}) {
    this.target = options.target ?? process.env.PC_GRPC_TARGET ?? DEFAULT_TARGET;
    this.token = options.token ?? process.env.PC_AUTH_TOKEN;
    this.author = options.author ?? process.env.PC_AUTHOR;
    this.timeoutMs = options.timeoutMs ?? 30_000;
    const creds =
      options.credentials ?? (options.secure ? credentials.createSsl() : credentials.createInsecure());
    this.stub = new pb.PostCortexClient(this.target, creds);
  }

  close(): void {
    this.stub.close();
  }

  private call<Req, Res>(method: keyof pb.PostCortexClient, request: Req): Promise<Res> {
    const metadata = new Metadata();
    if (this.token) metadata.set("authorization", `Bearer ${this.token}`);
    const fn = (this.stub[method] as unknown as Unary<Req, Res>).bind(this.stub);
    return new Promise((resolve, reject) => {
      fn(request, metadata, { deadline: new Date(Date.now() + this.timeoutMs) }, (error, response) =>
        error ? reject(toError(error)) : resolve(response),
      );
    });
  }

  // ---------------------------------------------------------------- health

  async health(): Promise<{
    healthy: boolean;
    version: string;
    activeSessions: number;
    totalUpdates: number;
    embeddingsEnabled: boolean;
  }> {
    const r = await this.call<pb.HealthRequest, pb.HealthResponse>("health", {});
    return {
      healthy: r.healthy,
      version: r.version,
      activeSessions: r.activeSessions,
      totalUpdates: r.totalUpdates,
      embeddingsEnabled: r.embeddingsEnabled,
    };
  }

  // -------------------------------------------------------------- sessions

  /** Create a session; returns its id. */
  async createSession(name = "", description = ""): Promise<string> {
    const r = await this.call<pb.CreateSessionRequest, pb.CreateSessionResponse>("createSession", {
      name,
      description,
    });
    return r.sessionId;
  }

  async listSessions(nameFilter = "", limit = 0): Promise<Session[]> {
    const r = await this.call<pb.ListSessionsRequest, pb.ListSessionsResponse>("listSessions", {
      nameFilter,
      limit,
    });
    return r.sessions.map(toSession);
  }

  async getSession(sessionId: string): Promise<Session> {
    const r = await this.call<pb.LoadSessionRequest, pb.LoadSessionResponse>("loadSession", {
      sessionId,
    });
    return toSession(r.session!);
  }

  /** Sessions whose name or description contains `query`. */
  async searchSessions(query: string): Promise<Session[]> {
    const r = await this.call<pb.SearchSessionsRequest, pb.SearchSessionsResponse>(
      "searchSessions",
      { query },
    );
    return r.sessions.map(toSession);
  }

  async updateSession(
    sessionId: string,
    changes: { name?: string; description?: string },
  ): Promise<boolean> {
    const r = await this.call<pb.UpdateSessionMetadataRequest, pb.UpdateSessionMetadataResponse>(
      "updateSessionMetadata",
      { sessionId, ...changes },
    );
    return r.success;
  }

  async deleteSession(sessionId: string): Promise<boolean> {
    const r = await this.call<pb.DeleteSessionRequest, pb.DeleteSessionResponse>("deleteSession", {
      sessionId,
    });
    return r.success;
  }

  // ------------------------------------------------------------ workspaces

  async createWorkspace(name: string, description = ""): Promise<string> {
    const r = await this.call<pb.CreateWorkspaceRequest, pb.CreateWorkspaceResponse>(
      "createWorkspace",
      { name, description },
    );
    return r.workspaceId;
  }

  /** The workspaces; archived ones only with `includeArchived`. */
  async listWorkspaces(includeArchived = false): Promise<Workspace[]> {
    const r = await this.call<pb.ListWorkspacesRequest, pb.ListWorkspacesResponse>(
      "listWorkspaces",
      { includeArchived },
    );
    return r.workspaces.map(toWorkspace);
  }

  /** Rename a workspace and/or change its description; returns it as it is now. */
  async updateWorkspace(
    workspaceId: string,
    changes: { name?: string; description?: string },
  ): Promise<Workspace> {
    const r = await this.call<pb.UpdateWorkspaceRequest, pb.UpdateWorkspaceResponse>(
      "updateWorkspace",
      { workspaceId, ...changes },
    );
    return toWorkspace(r.workspace!);
  }

  /** Archive a workspace (kept, left out of {@link listWorkspaces}); returns whether it changed. */
  async archiveWorkspace(workspaceId: string): Promise<boolean> {
    const r = await this.call<pb.ArchiveWorkspaceRequest, pb.ArchiveWorkspaceResponse>(
      "archiveWorkspace",
      { workspaceId },
    );
    return r.changed;
  }

  async unarchiveWorkspace(workspaceId: string): Promise<boolean> {
    const r = await this.call<pb.ArchiveWorkspaceRequest, pb.ArchiveWorkspaceResponse>(
      "unarchiveWorkspace",
      { workspaceId },
    );
    return r.changed;
  }

  async getWorkspace(workspaceId: string): Promise<Workspace> {
    const r = await this.call<pb.GetWorkspaceRequest, pb.GetWorkspaceResponse>("getWorkspace", {
      workspaceId,
    });
    return toWorkspace(r.workspace!);
  }

  async deleteWorkspace(workspaceId: string): Promise<boolean> {
    const r = await this.call<pb.DeleteWorkspaceRequest, pb.DeleteWorkspaceResponse>(
      "deleteWorkspace",
      { workspaceId },
    );
    return r.success;
  }

  /** Add a session to a workspace (`primary`, `related`, `dependency`, `shared`). */
  async addSession(workspaceId: string, sessionId: string, role = "primary"): Promise<boolean> {
    const r = await this.call<pb.AddSessionToWorkspaceRequest, pb.AddSessionToWorkspaceResponse>(
      "addSessionToWorkspace",
      { workspaceId, sessionId, role },
    );
    return r.success;
  }

  async removeSession(workspaceId: string, sessionId: string): Promise<boolean> {
    const r = await this.call<
      pb.RemoveSessionFromWorkspaceRequest,
      pb.RemoveSessionFromWorkspaceResponse
    >("removeSessionFromWorkspace", { workspaceId, sessionId });
    return r.success;
  }

  /**
   * The workspace and session of project `key`, created on first use. `key` is a normalised
   * git remote (`github.com/acme/api`) or `local:<hash>`; the same key always gets the same
   * workspace and session. A new key gets a session named `name` in the workspace named
   * `workspace` (created if missing), else in a new workspace named `name`. `client` names
   * the caller in the daemon's logs.
   */
  async resolveProject(
    key: string,
    name: string,
    options: { client?: string; workspace?: string } = {},
  ): Promise<Project> {
    const r = await this.call<pb.ResolveProjectRequest, pb.ResolveProjectResponse>(
      "resolveProject",
      pb.ResolveProjectRequest.fromPartial({
        key,
        name,
        client: options.client,
        workspace: options.workspace,
      }),
    );
    return { workspaceId: r.workspaceId, sessionId: r.sessionId, created: r.created };
  }

  // --------------------------------------------------------------- records

  /** Store one record; returns its entry id. */
  async record(sessionId: string, record: RecordInput, author?: string): Promise<string> {
    const r = await this.call<pb.UpdateContextRequest, pb.UpdateContextResponse>(
      "updateContext",
      pb.UpdateContextRequest.fromPartial({
        sessionId,
        interactionType: record.kind,
        content: toContent(record),
        entities: record.entities.map(toEntity),
        relations: record.relations.map(toRelation),
        author: author ?? this.author,
        derivedFrom: record.derivedFrom ?? [],
        validFrom: record.validFrom,
        supersedes: record.supersedes ?? [],
        confidence: record.confidence,
      }),
    );
    return r.updateId;
  }

  /** Store several records in one call; returns their entry ids in order. */
  async recordMany(sessionId: string, records: RecordInput[], author?: string): Promise<string[]> {
    const r = await this.call<pb.BulkUpdateContextRequest, pb.BulkUpdateContextResponse>(
      "bulkUpdateContext",
      pb.BulkUpdateContextRequest.fromPartial({
        sessionId,
        author: author ?? this.author,
        updates: records.map((record) => ({
          interactionType: record.kind,
          content: toContent(record),
          entities: record.entities.map(toEntity),
          relations: record.relations.map(toRelation),
          derivedFrom: record.derivedFrom ?? [],
          validFrom: record.validFrom,
          supersedes: record.supersedes ?? [],
          confidence: record.confidence,
        })),
      }),
    );
    if (r.failureCount) {
      throw new InvalidArgument(
        status.INVALID_ARGUMENT,
        `${r.failureCount} of ${records.length} records were rejected`,
      );
    }
    return r.updateIds;
  }

  /** The session's records, optionally of one kind and after a time. */
  async records(
    sessionId: string,
    options: { kind?: RecordKind; limit?: number; after?: Date } = {},
  ): Promise<
    {
      id: string;
      kind: string;
      title: string;
      description: string;
      details: string[];
      timestamp: Date;
      entities: string[];
    }[]
  > {
    const r = await this.call<pb.QueryContextRequest, pb.QueryContextResponse>("queryContext", {
      sessionId,
      interactionType: options.kind ?? "",
      limit: options.limit ?? 50,
      afterUnix: options.after ? Math.floor(options.after.getTime() / 1000) : 0,
    });
    return r.updates.map((u) => ({
      id: u.id,
      kind: recordKind(u.interactionType),
      title: u.content?.title ?? "",
      description: u.content?.description ?? "",
      details: u.content?.details ?? [],
      timestamp: fromUnix(u.timestampUnix),
      entities: u.entities,
    }));
  }

  // ---------------------------------------------------------------- search

  /**
   * Hybrid search over a session, a workspace, or everything (neither given). Hits come
   * with their neighbouring records (`neighbourOf` set); records that no longer hold carry
   * `validity` (its `validTo` and `reason`); `trace: true` fills `signals`.
   */
  async search(query: string, options: SearchOptions = {}): Promise<pb.SearchResult[]> {
    const r = await this.call<pb.SemanticSearchRequest, pb.SemanticSearchResponse>(
      "semanticSearch",
      pb.SemanticSearchRequest.fromPartial({
        query,
        maxResults: options.limit ?? 10,
        sessionId: options.sessionId ?? "",
        workspaceId: options.workspaceId,
        minScore: options.minScore ?? 0,
        mode: options.mode,
        sources: options.sources,
        author: options.author,
        preferAuthor: options.preferAuthor,
        trace: options.trace || undefined,
      }),
    );
    return r.results;
  }

  /**
   * Context for a prompt within `tokenBudget`: cards, the entities the query names, their
   * graph neighbourhood and dependents, records and (with `sources`) conversation excerpts.
   */
  async assemble(
    query: string,
    options: Scope & { tokenBudget?: number; sources?: Sources } = {},
  ): Promise<{
    formattedText: string;
    totalTokens: number;
    items: pb.AssembledContextItem[];
    entityContext: pb.AssembledEntityContext[];
    impact: pb.ImpactEntry[];
    cards: Card[];
  }> {
    const r = await this.call<pb.AssembleContextRequest, pb.AssembleContextResponse>(
      "assembleContext",
      pb.AssembleContextRequest.fromPartial({
        query,
        sessionId: options.sessionId ?? "",
        workspaceId: options.workspaceId,
        tokenBudget: options.tokenBudget ?? 4000,
        sources: options.sources,
      }),
    );
    return {
      formattedText: r.formattedText,
      totalTokens: r.totalTokens,
      items: r.items,
      entityContext: r.entityContext,
      impact: r.impact,
      cards: r.cards.map(toCard),
    };
  }

  // ----------------------------------------------------------- graph views

  /** `GetGraphView` of a session or a workspace, asking for one view. */
  private graphView(scope: Scope, view: pb.GetGraphViewRequest["view"]): Promise<pb.GraphView> {
    return this.call<pb.GetGraphViewRequest, pb.GraphView>("getGraphView", {
      sessionId: scope.sessionId ?? "",
      workspaceId: scope.workspaceId ?? "",
      view,
    });
  }

  /** An entity by name or alias: relations, records, decisions. */
  async entity(name: string, scope: Scope, limit = 100): Promise<pb.EntityView> {
    const r = await this.graphView(scope, { $case: "entity", entity: { entity: name, limit } });
    return r.view?.$case === "entity" ? r.view.entity : missingView("entity");
  }

  /** What depends on an entity, and the valid decisions, requirements, problems. */
  async impact(
    name: string,
    scope: Scope,
    options: { depth?: number; limit?: number } = {},
  ): Promise<pb.ImpactView> {
    const impact = { entity: name, depth: options.depth ?? 3, limit: options.limit ?? 100 };
    const r = await this.graphView(scope, { $case: "impact", impact });
    return r.view?.$case === "impact" ? r.view.impact : missingView("impact");
  }

  /** Problem -> decision -> consequences around an entity, in time order. */
  async lineage(name: string, scope: Scope, depth = 3): Promise<pb.LineageView> {
    const r = await this.graphView(scope, { $case: "lineage", lineage: { entity: name, depth } });
    return r.view?.$case === "lineage" ? r.view.lineage : missingView("lineage");
  }

  /** Entity communities with their central entities and key decisions. */
  async topics(
    scope: Scope,
    options: { limit?: number; perTopic?: number } = {},
  ): Promise<pb.TopicsView> {
    const topics = { limit: options.limit ?? 100, perTopic: options.perTopic ?? 5 };
    const r = await this.graphView(scope, { $case: "topics", topics });
    return r.view?.$case === "topics" ? r.view.topics : missingView("topics");
  }

  /**
   * Unsolved problems, decisions without rationale, open questions, stale entities. Items the
   * workspace dismissed are left out (and listed under `dismissed`) unless `includeDismissed`.
   */
  async radar(scope: Scope, limit = 100, includeDismissed = false): Promise<pb.RadarView> {
    const r = await this.graphView(scope, { $case: "radar", radar: { limit, includeDismissed } });
    return r.view?.$case === "radar" ? r.view.radar : missingView("radar");
  }

  /** Hide a radar item from the radar of the workspace and of its sessions. */
  async dismissRadarItem(
    workspaceId: string,
    kind: RadarKind,
    key: string,
    author?: string,
  ): Promise<RadarDismissal> {
    const r = await this.call<pb.DismissRadarItemRequest, pb.DismissRadarItemResponse>(
      "dismissRadarItem",
      { workspaceId, kind, key, author: author ?? this.author },
    );
    return toDismissal(r.dismissal!);
  }

  /** Show a dismissed radar item again; returns whether it was dismissed. */
  async restoreRadarItem(workspaceId: string, kind: RadarKind, key: string): Promise<boolean> {
    const r = await this.call<pb.RestoreRadarItemRequest, pb.RestoreRadarItemResponse>(
      "restoreRadarItem",
      { workspaceId, kind, key },
    );
    return r.existed;
  }

  async listRadarDismissals(workspaceId: string): Promise<RadarDismissal[]> {
    const r = await this.call<pb.ListRadarDismissalsRequest, pb.ListRadarDismissalsResponse>(
      "listRadarDismissals",
      { workspaceId },
    );
    return r.dismissals.map(toDismissal);
  }

  /** Past problems, decisions and dependents of a source file. */
  async fileContext(
    path: string,
    scope: Scope,
    options: { depth?: number; limit?: number } = {},
  ): Promise<pb.FileView> {
    const file = { path, depth: options.depth ?? 2, limit: options.limit ?? 20 };
    const r = await this.graphView(scope, { $case: "file", file });
    return r.view?.$case === "file" ? r.view.file : missingView("file");
  }

  // ---------------------------------------------- entity and record upkeep

  async deleteEntity(sessionId: string, name: string): Promise<boolean> {
    const r = await this.call<pb.DeleteEntityRequest, pb.DeleteEntityResponse>("deleteEntity", {
      sessionId,
      entityName: name,
    });
    return r.existed;
  }

  /** Make `name` an alias of `into`; mentions, relations and counts move over. */
  async mergeEntities(sessionId: string, name: string, into: string): Promise<boolean> {
    const r = await this.call<pb.MergeEntitiesRequest, pb.MergeEntitiesResponse>("mergeEntities", {
      sessionId,
      entityName: name,
      into,
    });
    return r.merged;
  }

  /** The record stops holding now; it stays in the history. */
  async forget(sessionId: string, entryId: string): Promise<boolean> {
    const r = await this.call<pb.UpdateRefRequest, pb.UpdateRefResponse>("forgetUpdate", {
      sessionId,
      entryId,
    });
    return r.existed;
  }

  /** Erase the record and everything derived from it. Cannot be undone. */
  async purge(sessionId: string, entryId: string): Promise<boolean> {
    const r = await this.call<pb.UpdateRefRequest, pb.UpdateRefResponse>("purgeUpdate", {
      sessionId,
      entryId,
    });
    return r.existed;
  }

  // ----------------------------------------------------------------- cards

  /** The project card of `workspaceId`, or the user card; with older versions. */
  async getCard(
    options: { workspaceId?: string; history?: number } = {},
  ): Promise<{ card?: Card; history: Card[] }> {
    const r = await this.call<pb.GetCardRequest, pb.GetCardResponse>(
      "getCard",
      pb.GetCardRequest.fromPartial({
        workspaceId: options.workspaceId,
        user: !options.workspaceId,
        history: options.history ?? 0,
      }),
    );
    return { card: r.card ? toCard(r.card) : undefined, history: r.history.map(toCard) };
  }

  /**
   * Write the next version (at most 2 KB). `expectedVersion` is the version the text was
   * edited from (0 for a new card); if the card changed since, nothing is written and the
   * latest version comes back to merge with.
   */
  async updateCard(
    content: string,
    options: { expectedVersion: number; workspaceId?: string; author?: string },
  ): Promise<CardWrite> {
    const r = await this.call<pb.UpdateCardRequest, pb.UpdateCardResponse>(
      "updateCard",
      pb.UpdateCardRequest.fromPartial({
        content,
        expectedVersion: options.expectedVersion,
        workspaceId: options.workspaceId,
        user: !options.workspaceId,
        author: options.author ?? this.author,
      }),
    );
    return { written: r.written, card: r.card ? toCard(r.card) : undefined };
  }

  // -------------------------------------------------------------- messages

  /** Append captured conversation turns; idempotent on each `turnId`. */
  async appendMessages(
    sessionId: string,
    messages: MessageInput[],
  ): Promise<{ appended: number[]; skipped: number; vectorized: number }> {
    const r = await this.call<pb.AppendMessagesRequest, pb.AppendMessagesResponse>(
      "appendMessages",
      pb.AppendMessagesRequest.fromPartial({
        sessionId,
        messages: messages.map((m) => ({
          turnId: m.turnId,
          role: m.role,
          text: m.text,
          author: m.author ?? this.author,
          sourceSession: m.sourceSession,
          timestampUnixMs: m.timestamp?.getTime(),
          tools: m.tools ?? [],
          files: m.files ?? [],
        })),
      }),
    );
    return { appended: r.appended, skipped: r.skipped, vectorized: r.vectorized };
  }

  // ------------------------------------------------------------- summaries

  /** Decisions, insights, entities and questions of a session (parsed JSON). */
  /** Delete the captured turn at `seq` (`message:<seq>`); records drawn from it keep the id. */
  async deleteMessage(sessionId: string, seq: number): Promise<MessageDeletion> {
    const r = await this.call<pb.DeleteMessageRequest, pb.DeleteMessagesResponse>("deleteMessage", {
      sessionId,
      seq,
    });
    return { deleted: r.deleted, remaining: r.remaining };
  }

  /** Delete the session's whole captured message stream; its records stay. */
  async deleteMessages(sessionId: string): Promise<MessageDeletion> {
    const r = await this.call<pb.DeleteMessagesRequest, pb.DeleteMessagesResponse>(
      "deleteMessages",
      { sessionId },
    );
    return { deleted: r.deleted, remaining: r.remaining };
  }

  // ------------------------------------------------------- export / import

  private metadata(): Metadata {
    const metadata = new Metadata();
    if (this.token) metadata.set("authorization", `Bearer ${this.token}`);
    return metadata;
  }

  /**
   * Export a scope as the bytes of a `pcx export` file (readable by `pcx import` and
   * {@link importArchive}), collected from the streamed chunks.
   */
  export(options: ExportOptions = {}): Promise<Archive> {
    const request: pb.ExportRequest = {
      scope: options.scope ?? "all",
      sessionIds: options.sessionIds ?? [],
      workspaceId: options.workspaceId ?? "",
      includeCheckpoints: options.includeCheckpoints ?? false,
      compression: options.compression ?? "none",
      noMessages: options.noMessages ?? false,
    };
    const stream = this.stub.export(request, this.metadata(), {
      deadline: new Date(Date.now() + this.timeoutMs),
    });
    return new Promise((resolve, reject) => {
      const parts: Buffer[] = [];
      let summary: pb.ExportSummary | undefined;
      stream.on("data", (chunk: pb.ExportChunk) => {
        parts.push(Buffer.from(chunk.data));
        if (chunk.last && chunk.summary) summary = chunk.summary;
      });
      stream.on("error", (e: ServiceError) => reject(toError(e)));
      stream.on("end", () => {
        if (!summary) {
          reject(new PostCortexError(status.DATA_LOSS, "export ended without a summary"));
          return;
        }
        resolve({ data: Buffer.concat(parts), summary: { ...summary } });
      });
    });
  }

  /**
   * Send a `pcx export` file (JSON, gzip or zstd) in chunks. Without `commit` nothing is
   * written and the result is a preview (counts, conflicts).
   */
  importArchive(data: Uint8Array, options: ImportOptions = {}): Promise<ImportResult> {
    const chunkSize = options.chunkSize ?? IMPORT_CHUNK_BYTES;
    if (chunkSize <= 0) throw new Error("chunkSize must be positive");
    const settings: pb.ImportSettings = {
      commit: options.commit ?? false,
      skipExisting: options.skipExisting ?? false,
      overwrite: options.overwrite ?? false,
      sessionIds: options.sessionIds ?? [],
      workspaceIds: options.workspaceIds ?? [],
      cardsNewer: options.cardsNewer ?? false,
    };
    const bytes = Buffer.from(data.buffer, data.byteOffset, data.byteLength);
    return new Promise((resolve, reject) => {
      const call = this.stub.import(
        this.metadata(),
        { deadline: new Date(Date.now() + this.timeoutMs) },
        (error, r) => (error ? reject(toError(error)) : resolve(toImportResult(r))),
      );
      call.write({ data: bytes.subarray(0, chunkSize), settings });
      for (let start = chunkSize; start < bytes.length; start += chunkSize) {
        call.write({ data: bytes.subarray(start, start + chunkSize) });
      }
      call.end();
    });
  }

  /** Decisions, insights, entities and questions of a session. */
  structuredSummary(sessionId: string, compact = true): Promise<pb.StructuredSummary> {
    return this.call<pb.GetStructuredSummaryRequest, pb.StructuredSummary>(
      "getStructuredSummary",
      pb.GetStructuredSummaryRequest.fromPartial({ sessionId, compact }),
    );
  }

  /** The session's key decisions. */
  async keyDecisions(sessionId: string): Promise<pb.DecisionSummary[]> {
    const r = await this.call<pb.GetKeyDecisionsRequest, pb.KeyDecisions>("getKeyDecisions", {
      sessionId,
    });
    return r.decisions;
  }

  /** The session's counts. */
  sessionStatistics(sessionId: string): Promise<pb.SessionStatistics> {
    return this.call<pb.GetSessionStatisticsRequest, pb.SessionStatistics>(
      "getSessionStatistics",
      { sessionId },
    );
  }

  // ----------------------------------------------------------------- admin

  /** Embed what a session has not embedded yet; returns the vectors created. */
  async vectorizeSession(sessionId: string): Promise<number> {
    const r = await this.call<pb.VectorizeSessionRequest, pb.VectorizeSessionResponse>(
      "vectorizeSession",
      { sessionId },
    );
    return r.vectorsCreated;
  }

  async createCheckpoint(sessionId: string): Promise<string> {
    const r = await this.call<pb.CreateCheckpointRequest, pb.CreateCheckpointResponse>(
      "createCheckpoint",
      { sessionId },
    );
    return r.checkpointId;
  }
}

// ------------------------------------------------------------------ mapping

function toSession(s: pb.SessionInfo): Session {
  return {
    id: s.sessionId,
    name: s.name,
    description: s.description,
    createdAt: fromUnix(s.createdAtUnix),
    updateCount: s.updateCount,
  };
}

function toWorkspace(w: pb.WorkspaceInfo): Workspace {
  return {
    id: w.workspaceId,
    name: w.name,
    description: w.description,
    createdAt: fromUnix(w.createdAtUnix),
    sessions: Object.fromEntries(w.sessions.map((e) => [e.sessionId, e.role.toLowerCase()])),
    ...(w.archived ? { archivedAt: fromUnix(w.archivedAtUnix) } : {}),
  };
}

function toDismissal(d: pb.RadarDismissal): RadarDismissal {
  return {
    kind: d.kind as RadarKind,
    key: d.key,
    dismissedAt: fromUnix(d.dismissedAtUnix),
    ...(d.author !== undefined ? { author: d.author } : {}),
  };
}

function toImportResult(r: pb.ImportResponse): ImportResult {
  return {
    committed: r.committed,
    formatVersion: r.formatVersion,
    sessions: r.sessions,
    workspaces: r.workspaces,
    updates: r.updates,
    checkpoints: r.checkpoints,
    embeddings: r.embeddings,
    conflicts: r.conflicts.map((c) => ({
      kind: c.kind as ImportConflict["kind"],
      id: c.id,
      name: c.name,
      resolution: c.resolution as ImportConflict["resolution"],
      target: c.target ? cardHead(c.target) : undefined,
      archive: c.archive ? cardHead(c.archive) : undefined,
    })),
    sessionsImported: r.sessionsImported,
    sessionsSkipped: r.sessionsSkipped,
    workspacesImported: r.workspacesImported,
    workspacesSkipped: r.workspacesSkipped,
    updatesImported: r.updatesImported,
    checkpointsImported: r.checkpointsImported,
    embeddingsImported: r.embeddingsImported,
    errors: r.errors,
    messages: r.messages,
    messagesNew: r.messagesNew,
    cards: r.cards,
    messagesImported: r.messagesImported,
    messagesSkipped: r.messagesSkipped,
    cardsImported: r.cardsImported,
    cardsSkipped: r.cardsSkipped,
  };
}

function toContent(r: RecordInput): pb.ContextContent {
  return pb.ContextContent.fromPartial({
    title: r.title,
    description: r.description ?? "",
    details: r.details ?? [],
    examples: r.examples ?? [],
    implications: r.implications ?? [],
    codeRef: r.codeRef
      ? {
          filePath: r.codeRef.filePath,
          startLine: r.codeRef.startLine ?? 0,
          endLine: r.codeRef.endLine ?? 0,
          codeSnippet: r.codeRef.codeSnippet ?? "",
          commitHash: r.codeRef.commitHash ?? "",
          branch: r.codeRef.branch ?? "",
          changeDescription: r.codeRef.changeDescription ?? "",
        }
      : undefined,
  });
}

function toEntity(e: Entity): pb.EntityInfo {
  return { name: e.name, entityType: e.type ?? "concept" };
}

function toRelation(r: Relation): pb.RelationInfo {
  return { fromEntity: r.from, toEntity: r.to, relationType: r.type, context: r.context };
}

function toCard(c: pb.CardInfo): Card {
  return {
    scope: c.scope,
    version: c.version,
    text: c.text,
    updatedAt: fromUnix(c.updatedAtUnix),
    author: c.author,
  };
}
