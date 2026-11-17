// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The capability's detail views as `pcx.views.v1` messages. Enums go out
//! by their wire names (`as_str`); nothing is computed here.

use chrono::{DateTime, Utc};
use post_cortex_core::core::dismissal::RadarDismissal;
use post_cortex_core::graph::temporal::{EndReason, Validity};
use post_cortex_memory::capabilities::SearchDetail;
use post_cortex_memory::graph_views::detail::{
    EdgeDetail, EntityDetail, EventKind, FileDetail, FileTouch, Files, Found, FoundItem,
    GraphDetail, NodeDetail, Overview, RadarDetail, RecordDetail, RecordPage, SearchHitDetail,
    Timeline, TopicsDetail,
};
use post_cortex_memory::graph_views::{AliasCandidate, ImpactItem, RelationRef};
use post_cortex_proto::views as pb;
use tonic::Status;
use uuid::Uuid;

use crate::views::streams::message;

/// A count on the wire (saturating: a view never holds `u32::MAX` items).
pub fn n(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// A time on the wire.
pub fn timestamp(t: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: t.timestamp(),
        nanos: i32::try_from(t.timestamp_subsec_nanos()).unwrap_or(i32::MAX),
    }
}

/// Ids on the wire.
pub fn ids(ids: &[Uuid]) -> Vec<String> {
    ids.iter().map(Uuid::to_string).collect()
}

fn end_reason(reason: Option<EndReason>) -> pb::EndReason {
    match reason {
        None => pb::EndReason::Unspecified,
        Some(EndReason::Superseded) => pb::EndReason::Superseded,
        Some(EndReason::Contradicted) => pb::EndReason::Contradicted,
        Some(EndReason::Restated) => pb::EndReason::Restated,
        Some(EndReason::Duplicate) => pb::EndReason::Duplicate,
        Some(EndReason::Forgotten) => pb::EndReason::Forgotten,
    }
}

/// When a record held.
pub fn validity(v: Validity) -> pb::Validity {
    pb::Validity {
        valid_from: Some(timestamp(v.valid_from)),
        valid_to: v.valid_to.map(timestamp),
        reason: end_reason(v.reason) as i32,
        ended_by: v.ended_by.map(|id| id.to_string()),
    }
}

fn relation_ref(r: RelationRef) -> pb::RelationRef {
    pb::RelationRef {
        from: r.from,
        to: r.to,
        relation_type: r.relation_type.as_str().to_string(),
    }
}

/// A record in full.
pub fn record(r: RecordDetail) -> pb::Record {
    pb::Record {
        id: r.id.to_string(),
        session_id: r.session_id.to_string(),
        session_name: r.session_name,
        interaction_type: r.update_type.as_str().to_string(),
        title: r.title,
        description: r.description,
        details: r.details,
        examples: r.examples,
        implications: r.implications,
        author: r.author,
        created_at: Some(timestamp(r.created_at)),
        validity: Some(validity(r.validity)),
        confidence: r.confidence,
        entities: r
            .entities
            .into_iter()
            .map(|e| pb::EntityRef {
                name: e.name,
                entity_type: e.entity_type.map(|t| t.as_str().to_string()),
            })
            .collect(),
        relations: r
            .relations
            .into_iter()
            .map(|rel| pb::RecordRelation {
                from: rel.relation.from,
                to: rel.relation.to,
                relation_type: rel.relation.relation_type.as_str().to_string(),
                context: rel.context,
            })
            .collect(),
        code: r.code.map(|c| pb::CodeRef {
            file_path: c.file_path,
            start_line: c.start_line,
            end_line: c.end_line,
            commit: c.commit_hash,
            branch: c.branch,
            change_description: c.change_description,
            snippet: c.code_snippet,
        }),
        derived_from: r.derived_from,
        supersedes: ids(&r.supersedes),
        ended: ids(&r.ended),
        important: r.important,
    }
}

fn records(rs: Vec<RecordDetail>) -> Vec<pb::Record> {
    rs.into_iter().map(record).collect()
}

fn node(d: NodeDetail) -> pb::Node {
    let n_ = d.node;
    pb::Node {
        name: n_.name,
        entity_type: n_.entity_type,
        aliases: n_.aliases,
        description: n_.description,
        importance: n_.importance,
        mention_count: n_.mention_count,
        records: n(n_.records),
        first_seen: Some(timestamp(n_.first_seen)),
        last_seen: Some(timestamp(n_.last_seen)),
        community: n_.community.map(n),
        authors: d.provenance.authors,
        session_ids: ids(&d.provenance.sessions),
    }
}

fn nodes(ds: Vec<NodeDetail>) -> Vec<pb::Node> {
    ds.into_iter().map(node).collect()
}

fn edge(d: EdgeDetail) -> pb::Edge {
    let e = d.edge;
    pb::Edge {
        from: e.from,
        to: e.to,
        relation_type: e.relation_type,
        context: e.context,
        weight: e.weight,
        valid_from: e.valid_from.map(timestamp),
        valid_to: e.valid_to.map(timestamp),
        records: ids(&e.records),
        authors: d.provenance.authors,
        session_ids: ids(&d.provenance.sessions),
    }
}

/// The graph.
pub fn graph(g: GraphDetail) -> pb::Graph {
    pb::Graph {
        nodes: nodes(g.nodes),
        edges: g.edges.into_iter().map(edge).collect(),
        total_nodes: n(g.total_nodes),
        total_edges: n(g.total_edges),
        at: g.at.map(timestamp),
    }
}

/// The timeline.
pub fn timeline(t: Timeline) -> pb::Timeline {
    pb::Timeline {
        events: t
            .events
            .into_iter()
            .map(|e| pb::TimelineEvent {
                at: Some(timestamp(e.at)),
                kind: match e.kind {
                    EventKind::Recorded => pb::EventKind::Recorded,
                    EventKind::Ended => pb::EventKind::Ended,
                    EventKind::RelationAdded => pb::EventKind::RelationAdded,
                    EventKind::RelationEnded => pb::EventKind::RelationEnded,
                } as i32,
                record: e.record.to_string(),
                session_id: e.session_id.to_string(),
                title: e.title,
                interaction_type: e.update_type.as_str().to_string(),
                relation: e.relation.map(relation_ref),
                reason: end_reason(e.reason) as i32,
            })
            .collect(),
        total: n(t.total),
    }
}

/// A page of the browser.
pub fn record_page(p: RecordPage) -> pb::RecordPage {
    pb::RecordPage {
        records: records(p.records),
        total: n(p.total),
        next_cursor: p.next.map(|c| c.to_string()).unwrap_or_default(),
    }
}

fn file_touch(f: FileTouch) -> pb::FileTouch {
    pb::FileTouch {
        path: f.path,
        records: n(f.records),
        last_interaction_type: f.last_update_type.as_str().to_string(),
        last_at: Some(timestamp(f.last_at)),
    }
}

/// The files of a scope.
pub fn files(f: Files) -> pb::Files {
    pb::Files {
        files: f.files.into_iter().map(file_touch).collect(),
        total: n(f.total),
        next_cursor: f.next.map(|c| c.to_string()).unwrap_or_default(),
    }
}

/// The overview.
pub fn overview(o: Overview) -> pb::Overview {
    pb::Overview {
        records: n(o.records),
        records_day: n(o.records_day),
        records_week: n(o.records_week),
        current_records: n(o.current_records),
        entities: n(o.entities),
        relations: n(o.relations),
        topics: n(o.topics),
        messages: o.messages,
        sessions: n(o.sessions),
        active_sessions: n(o.active_sessions),
        authors: o
            .authors
            .into_iter()
            .map(|a| pb::AuthorCount {
                author: a.author,
                records: n(a.records),
            })
            .collect(),
        first_activity: o.first_activity.map(timestamp),
        last_activity: o.last_activity.map(timestamp),
        top_entities: nodes(o.top_entities),
        recent_decisions: records(o.recent_decisions),
        recent_records: records(o.recent_records),
        radar: Some(pb::RadarSummary {
            unsolved_problems: n(o.radar.unsolved_problems),
            decisions_without_rationale: n(o.radar.decisions_without_rationale),
            open_questions: n(o.radar.open_questions),
            stale_entities: n(o.radar.stale_entities),
            alias_candidates: n(o.radar.alias_candidates),
        }),
        embedding_queue: n(o.embedding_queue),
        session_stats: o
            .session_stats
            .into_iter()
            .map(|s| pb::SessionStat {
                session_id: s.session_id.to_string(),
                name: s.name,
                records: n(s.records),
                last_record: s.last_record.map(timestamp),
            })
            .collect(),
    }
}

fn dependents(items: Vec<ImpactItem>) -> Vec<pb::Dependent> {
    items
        .into_iter()
        .map(|d| pb::Dependent {
            entity: d.entity,
            entity_type: d.entity_type,
            via: d.via,
            relation_type: d.relation_type,
            context: d.context,
            depth: n(d.depth),
        })
        .collect()
}

/// An entity's page.
pub fn entity(e: EntityDetail) -> pb::EntityDetail {
    pb::EntityDetail {
        entity: Some(node(e.entity)),
        rank: n(e.rank),
        ranked: n(e.ranked),
        topic_size: n(e.topic_size),
        relations: e.relations.into_iter().map(edge).collect(),
        records: records(e.records),
        total_records: n(e.total_records),
        decisions: records(e.decisions),
        impact: Some(pb::Impact {
            entity: e.impact.entity,
            depth: n(e.impact.depth),
            dependents: dependents(e.impact.dependents),
            risks: e
                .impact
                .risks
                .into_iter()
                .map(|r| pb::Risk {
                    record: Some(record(r.record)),
                    about: r.about,
                    depth: n(r.depth),
                })
                .collect(),
            total_risks: n(e.impact.total_risks),
        }),
        lineage: Some(pb::Lineage {
            entity: e.lineage.entity,
            chain: e.lineage.chain,
            steps: e
                .lineage
                .steps
                .into_iter()
                .map(|s| pb::LineageStep {
                    record: Some(record(s.record)),
                    relation: s.relation.map(relation_ref),
                })
                .collect(),
        }),
        files: e.files.into_iter().map(file_touch).collect(),
    }
}

/// The topics.
pub fn topics(t: TopicsDetail) -> pb::Topics {
    pb::Topics {
        topics: t
            .topics
            .into_iter()
            .map(|t| pb::Topic {
                id: n(t.id),
                size: n(t.size),
                central: nodes(t.central),
                key_decisions: records(t.key_decisions),
                records: n(t.records),
                first: t.first.map(timestamp),
                last: t.last.map(timestamp),
            })
            .collect(),
        unclustered: n(t.unclustered),
    }
}

fn dismissal(d: RadarDismissal) -> pb::RadarDismissal {
    pb::RadarDismissal {
        kind: d.kind.as_str().to_string(),
        key: d.key,
        dismissed_at: Some(timestamp(d.dismissed_at)),
        author: d.author,
    }
}

/// A name that may be another entity's alias.
pub fn alias(a: AliasCandidate) -> pb::AliasCandidate {
    pb::AliasCandidate {
        name: a.name,
        of: a.of,
        shared_records: n(a.shared_records),
    }
}

/// The radar.
pub fn radar(r: RadarDetail) -> pb::Radar {
    pb::Radar {
        unsolved_problems: nodes(r.unsolved_problems),
        decisions_without_rationale: records(r.decisions_without_rationale),
        open_questions: r.open_questions,
        stale_entities: nodes(r.stale_entities),
        alias_candidates: r.alias_candidates.into_iter().map(alias).collect(),
        dismissed: r.dismissed.into_iter().map(dismissal).collect(),
    }
}

/// A file view.
pub fn file(f: FileDetail) -> pb::FileView {
    pb::FileView {
        path: f.path,
        records: records(f.records),
        problems: records(f.problems),
        decisions: records(f.decisions),
        entities: nodes(f.entities),
        dependents: dependents(f.dependents),
        changed_with: nodes(f.changed_with),
    }
}

fn item(i: FoundItem) -> Result<pb::SearchItem, Status> {
    Ok(pb::SearchItem {
        content_id: i.content_id,
        session_id: i.session_id.to_string(),
        content_type: i.content_type.as_str().to_string(),
        text: i.text,
        timestamp: Some(timestamp(i.timestamp)),
        author: i.author,
        source: i
            .found
            .map(|found| {
                Ok::<_, Status>(match found {
                    Found::Record(r) => pb::search_item::Source::Record(Box::new(record(*r))),
                    Found::Turn(t) => pb::search_item::Source::Message(Box::new(message(*t)?)),
                })
            })
            .transpose()?,
    })
}

fn hit(h: SearchHitDetail) -> Result<pb::SearchHit, Status> {
    Ok(pb::SearchHit {
        item: Some(item(h.item)?),
        score: h.score,
        similarity: h.similarity,
        importance: h.importance,
        trace: h.trace.map(|s| pb::SearchTrace {
            vector_rank: s.vector_rank.map(n),
            vector_similarity: s.vector_similarity,
            bm25_rank: s.bm25_rank.map(n),
            graph_rank: s.graph_rank.map(n),
            fused: s.fused,
            rerank: s.rerank,
        }),
        before: h.before.into_iter().map(item).collect::<Result<_, _>>()?,
        after: h.after.into_iter().map(item).collect::<Result<_, _>>()?,
        ended_by_title: h.ended_by_title,
    })
}

/// Search results.
///
/// # Errors
/// A turn whose blocks do not map.
pub fn search(s: SearchDetail) -> Result<pb::SearchResponse, Status> {
    Ok(pb::SearchResponse {
        hits: s.hits.into_iter().map(hit).collect::<Result<_, _>>()?,
        took_ms: u32::try_from(s.took_ms).unwrap_or(u32::MAX),
        neighbour_radius: n(s.neighbour_radius),
        reranked: s.reranked,
        rerank_skipped: s.rerank_skipped.map(|r| r.as_str().to_string()),
    })
}
