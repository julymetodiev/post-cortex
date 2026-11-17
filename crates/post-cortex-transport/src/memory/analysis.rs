// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The memory's summaries, analyses and graph views as `pcx.v1` messages
//! (W23): each field as the capability gives it; nothing is computed here.

use post_cortex_core::core::structured_context::QuestionStatus;
use post_cortex_core::graph::entity_graph::EntityAnalysis;
use post_cortex_core::summary::presentation::{
    ActivityLevel, ComplexityLevel, ConfidenceLevel, RecencyLevel, SessionDuration, UrgencyLevel,
};
use post_cortex_core::summary::{
    ConceptSummary, DecisionSummary, EntitySummary, ImportanceLevel, QuestionSummary, SessionStats,
};
use post_cortex_memory::capabilities::{
    Compaction, EntityImportance, EntityNetworkView, SessionStatistics, StructuredSummary,
    SummarySection,
};
use post_cortex_memory::content_vectorizer::VectorizationStats;
use post_cortex_memory::graph_views::{
    EntityView, FileView, GraphEdge, GraphNode, ImpactItem, ImpactView, LineageStep, LineageView,
    RadarView, RecordView, RenderedView, TopicView, TopicsView,
};
use post_cortex_proto::{pb, views};

use crate::memory::lifecycle::dismissal_info;
use crate::views::response::{alias, timestamp, validity};

/// A count on the wire.
fn n(count: usize) -> u64 {
    count as u64
}

/// The sections of a summary request on the wire.
pub fn section(section: pb::SummarySection) -> Option<SummarySection> {
    match section {
        pb::SummarySection::Unspecified => None,
        pb::SummarySection::Decisions => Some(SummarySection::Decisions),
        pb::SummarySection::Insights => Some(SummarySection::Insights),
        pb::SummarySection::Entities => Some(SummarySection::Entities),
        pb::SummarySection::Questions => Some(SummarySection::Questions),
    }
}

fn section_wire(section: SummarySection) -> pb::SummarySection {
    match section {
        SummarySection::Decisions => pb::SummarySection::Decisions,
        SummarySection::Insights => pb::SummarySection::Insights,
        SummarySection::Entities => pb::SummarySection::Entities,
        SummarySection::Questions => pb::SummarySection::Questions,
    }
}

fn confidence(level: &ConfidenceLevel) -> pb::ConfidenceLevel {
    match level {
        ConfidenceLevel::VeryLow => pb::ConfidenceLevel::VeryLow,
        ConfidenceLevel::Low => pb::ConfidenceLevel::Low,
        ConfidenceLevel::Medium => pb::ConfidenceLevel::Medium,
        ConfidenceLevel::High => pb::ConfidenceLevel::High,
    }
}

fn status(status: &QuestionStatus) -> pb::QuestionStatus {
    match status {
        QuestionStatus::Open => pb::QuestionStatus::Open,
        QuestionStatus::InProgress => pb::QuestionStatus::InProgress,
        QuestionStatus::Answered => pb::QuestionStatus::Answered,
        QuestionStatus::Deferred => pb::QuestionStatus::Deferred,
    }
}

fn urgency(level: &UrgencyLevel) -> pb::UrgencyLevel {
    match level {
        UrgencyLevel::Low => pb::UrgencyLevel::Low,
        UrgencyLevel::Medium => pb::UrgencyLevel::Medium,
        UrgencyLevel::High => pb::UrgencyLevel::High,
    }
}

fn complexity(level: &ComplexityLevel) -> pb::ComplexityLevel {
    match level {
        ComplexityLevel::Low => pb::ComplexityLevel::Low,
        ComplexityLevel::Medium => pb::ComplexityLevel::Medium,
        ComplexityLevel::High => pb::ComplexityLevel::High,
    }
}

fn importance_level(level: &ImportanceLevel) -> pb::ImportanceLevel {
    match level {
        ImportanceLevel::Minimal => pb::ImportanceLevel::Minimal,
        ImportanceLevel::Low => pb::ImportanceLevel::Low,
        ImportanceLevel::Medium => pb::ImportanceLevel::Medium,
        ImportanceLevel::High => pb::ImportanceLevel::High,
        ImportanceLevel::Critical => pb::ImportanceLevel::Critical,
    }
}

fn recency(level: &RecencyLevel) -> pb::RecencyLevel {
    match level {
        RecencyLevel::Today => pb::RecencyLevel::Today,
        RecencyLevel::Recent => pb::RecencyLevel::Recent,
        RecencyLevel::ThisWeek => pb::RecencyLevel::ThisWeek,
        RecencyLevel::ThisMonth => pb::RecencyLevel::ThisMonth,
        RecencyLevel::Old => pb::RecencyLevel::Old,
    }
}

fn duration(level: &SessionDuration) -> pb::SessionDuration {
    match level {
        SessionDuration::Short => pb::SessionDuration::Short,
        SessionDuration::Medium => pb::SessionDuration::Medium,
        SessionDuration::Long => pb::SessionDuration::Long,
        SessionDuration::Extended => pb::SessionDuration::Extended,
    }
}

fn activity(level: &ActivityLevel) -> pb::ActivityLevel {
    match level {
        ActivityLevel::Minimal => pb::ActivityLevel::Minimal,
        ActivityLevel::Low => pb::ActivityLevel::Low,
        ActivityLevel::Medium => pb::ActivityLevel::Medium,
        ActivityLevel::High => pb::ActivityLevel::High,
        ActivityLevel::VeryHigh => pb::ActivityLevel::VeryHigh,
    }
}

fn decision(d: DecisionSummary) -> pb::DecisionSummary {
    pb::DecisionSummary {
        description: d.description,
        context: d.context,
        alternatives: d.alternatives,
        confidence: d.confidence,
        timestamp: Some(timestamp(d.timestamp)),
        confidence_level: confidence(&d.confidence_level).into(),
    }
}

fn question(q: QuestionSummary) -> pb::QuestionSummary {
    pb::QuestionSummary {
        question: q.question,
        context: q.context,
        status: status(&q.status).into(),
        timestamp: Some(timestamp(q.timestamp)),
        last_updated: Some(timestamp(q.last_updated)),
        days_open: q.days_open,
        urgency_level: urgency(&q.urgency_level).into(),
    }
}

fn concept(c: ConceptSummary) -> pb::ConceptSummary {
    pb::ConceptSummary {
        name: c.name,
        definition: c.definition,
        examples: c.examples,
        related_concepts: c.related_concepts,
        timestamp: Some(timestamp(c.timestamp)),
        complexity_level: complexity(&c.complexity_level).into(),
    }
}

fn entity_summary(e: EntitySummary) -> pb::EntitySummary {
    pb::EntitySummary {
        entity_name: e.entity_name,
        importance_score: e.importance_score,
        mention_count: e.mention_count,
        relationship_count: n(e.relationship_count),
        first_seen: Some(timestamp(e.first_seen)),
        last_seen: Some(timestamp(e.last_seen)),
        importance_level: importance_level(&e.importance_level).into(),
        recency_level: recency(&e.recency_level).into(),
    }
}

fn summary_stats(s: SessionStats) -> pb::SummaryStats {
    pb::SummaryStats {
        session_id: s.session_id.to_string(),
        hot_context_size: n(s.hot_context_size),
        warm_context_size: n(s.warm_context_size),
        cold_context_size: n(s.cold_context_size),
        total_updates: n(s.total_updates),
        entity_count: n(s.entity_count),
        decision_count: n(s.decision_count),
        open_question_count: n(s.open_question_count),
        concept_count: n(s.concept_count),
        code_reference_count: n(s.code_reference_count),
        created_at: Some(timestamp(s.created_at)),
        last_updated: Some(timestamp(s.last_updated)),
        session_duration: duration(&s.session_duration).into(),
        activity_level: activity(&s.activity_level).into(),
    }
}

/// A structured summary.
pub fn summary(s: StructuredSummary) -> pb::StructuredSummary {
    let view = s.view;
    let (compaction, estimated_tokens) = match s.compaction {
        Compaction::None => (pb::Compaction::None, 0),
        Compaction::Requested => (pb::Compaction::Requested, 0),
        Compaction::Auto { estimated_tokens } => (pb::Compaction::Auto, n(estimated_tokens)),
    };
    pb::StructuredSummary {
        session_id: view.session_id.to_string(),
        generated_at: Some(timestamp(view.generated_at)),
        key_decisions: view.key_decisions.into_iter().map(decision).collect(),
        answered_questions: view.answered_questions.into_iter().map(question).collect(),
        key_concepts: view.key_concepts.into_iter().map(concept).collect(),
        important_entities: view.important_entities,
        entity_summaries: view
            .entity_summaries
            .into_iter()
            .map(entity_summary)
            .collect(),
        session_stats: Some(summary_stats(view.session_stats)),
        key_insights: s.insights.unwrap_or_default(),
        sections: s
            .sections
            .unwrap_or_default()
            .into_iter()
            .map(|s| section_wire(s).into())
            .collect(),
        compaction: compaction.into(),
        estimated_tokens,
    }
}

/// The key decisions of a session.
pub fn decisions(decisions: Vec<DecisionSummary>) -> pb::KeyDecisions {
    pb::KeyDecisions {
        decisions: decisions.into_iter().map(decision).collect(),
    }
}

fn analysis(a: EntityAnalysis) -> pb::EntityAnalysis {
    pb::EntityAnalysis {
        entity_name: a.entity_name,
        importance_score: a.importance_score,
        mention_count: a.mention_count,
        relationship_count: n(a.relationship_count),
        update_references: n(a.update_references),
        first_seen: Some(timestamp(a.first_seen)),
        last_seen: Some(timestamp(a.last_seen)),
    }
}

/// A session's entities ranked by importance.
pub fn importance(i: EntityImportance) -> pb::EntityImportance {
    pb::EntityImportance {
        entities: i.entities.into_iter().map(analysis).collect(),
        total_entities: n(i.pagination.total_entities),
        after_filter: n(i.pagination.after_filter),
        shown: n(i.pagination.shown),
        truncated: i.pagination.truncated,
    }
}

/// The entity network around a center.
pub fn network(v: EntityNetworkView) -> pb::EntityNetwork {
    pb::EntityNetwork {
        session_id: v.session_id,
        center_entity: v.center_entity,
        entities: v
            .entities
            .into_iter()
            .map(|e| pb::NetworkEntity {
                name: e.name,
                entity_type: e.entity_type.as_str().to_string(),
                importance_score: e.importance_score,
                mention_count: e.mention_count,
            })
            .collect(),
        relationships: v
            .relationships
            .into_iter()
            .map(|r| pb::NetworkRelationship {
                from: r.from,
                relation_type: r.relation_type.as_str().to_string(),
                to: r.to,
            })
            .collect(),
    }
}

/// Size and activity of a session.
pub fn statistics(s: SessionStatistics) -> pb::SessionStatistics {
    pb::SessionStatistics {
        session_id: s.session_id,
        name: s.name,
        description: s.description,
        created_at: Some(timestamp(s.created_at)),
        last_updated: Some(timestamp(s.last_updated)),
        hot_updates: n(s.hot_updates),
        total_updates: n(s.total_updates),
        entity_count: n(s.entity_count),
        relationship_count: n(s.relationship_count),
        code_references: n(s.code_references),
        change_history: n(s.change_history),
        first_update: s.first_update.map(timestamp),
        last_update: s.last_update.map(timestamp),
        duration_minutes: s.duration_minutes,
    }
}

/// Size of the search index.
pub fn vectorization(s: VectorizationStats) -> pb::VectorizationStats {
    pb::VectorizationStats {
        total_vectors: n(s.total_vectors),
        memory_usage_mb: n(s.memory_usage_mb),
        embedding_dimension: n(s.embedding_dimension),
    }
}

fn node(g: GraphNode) -> pb::GraphNode {
    pb::GraphNode {
        name: g.name,
        entity_type: g.entity_type,
        aliases: g.aliases,
        description: g.description,
        importance: g.importance,
        mention_count: g.mention_count,
        records: n(g.records),
        first_seen: Some(timestamp(g.first_seen)),
        last_seen: Some(timestamp(g.last_seen)),
        community: g.community.map(n),
    }
}

fn nodes(gs: Vec<GraphNode>) -> Vec<pb::GraphNode> {
    gs.into_iter().map(node).collect()
}

fn edge(e: GraphEdge) -> pb::GraphEdge {
    pb::GraphEdge {
        from: e.from,
        to: e.to,
        relation_type: e.relation_type,
        context: e.context,
        weight: e.weight,
        valid_from: e.valid_from.map(timestamp),
        valid_to: e.valid_to.map(timestamp),
        records: e.records.iter().map(ToString::to_string).collect(),
    }
}

fn record(r: RecordView) -> pb::RecordView {
    pb::RecordView {
        id: r.id.to_string(),
        session_id: r.session_id.to_string(),
        interaction_type: r.interaction_type,
        title: r.title,
        description: r.description,
        author: r.author,
        timestamp: Some(timestamp(r.timestamp)),
        confidence: r.confidence,
        derived_from: r.derived_from,
        file: r.file,
        validity: Some(validity(r.validity)),
    }
}

fn records(rs: Vec<RecordView>) -> Vec<pb::RecordView> {
    rs.into_iter().map(record).collect()
}

fn impact_item(i: ImpactItem) -> pb::ImpactItem {
    pb::ImpactItem {
        entity: i.entity,
        entity_type: i.entity_type,
        via: i.via,
        relation_type: i.relation_type,
        context: i.context,
        depth: n(i.depth),
    }
}

fn impact_items(is: Vec<ImpactItem>) -> Vec<pb::ImpactItem> {
    is.into_iter().map(impact_item).collect()
}

fn entity_view(v: EntityView) -> pb::EntityView {
    pb::EntityView {
        entity: Some(node(v.entity)),
        relations: v.relations.into_iter().map(edge).collect(),
        records: records(v.records),
        decisions: records(v.decisions),
        total_records: n(v.total_records),
    }
}

fn impact_view(v: ImpactView) -> pb::ImpactView {
    pb::ImpactView {
        entity: v.entity,
        dependents: impact_items(v.dependents),
        risks: v
            .risks
            .into_iter()
            .map(|r| pb::ImpactRisk {
                record: Some(record(r.record)),
                about: r.about,
                depth: n(r.depth),
            })
            .collect(),
        total_risks: n(v.total_risks),
    }
}

fn topic(t: TopicView) -> pb::TopicView {
    pb::TopicView {
        id: n(t.id),
        size: n(t.size),
        central: nodes(t.central),
        key_decisions: records(t.key_decisions),
        records: n(t.records),
        first: t.first.map(timestamp),
        last: t.last.map(timestamp),
    }
}

fn topics_view(v: TopicsView) -> pb::TopicsView {
    pb::TopicsView {
        topics: v.topics.into_iter().map(topic).collect(),
        unclustered: n(v.unclustered),
    }
}

fn lineage_step(s: LineageStep) -> pb::LineageStep {
    pb::LineageStep {
        record: Some(record(s.record)),
        relation: s
            .relation
            .map(|(from, to, relation_type)| views::RelationRef {
                from,
                to,
                relation_type,
            }),
    }
}

fn lineage_view(v: LineageView) -> pb::LineageView {
    pb::LineageView {
        entity: v.entity,
        chain: v.chain,
        steps: v.steps.into_iter().map(lineage_step).collect(),
    }
}

fn file_view(v: FileView) -> pb::FileView {
    pb::FileView {
        path: v.path,
        records: records(v.records),
        problems: records(v.problems),
        decisions: records(v.decisions),
        entities: nodes(v.entities),
        dependents: impact_items(v.dependents),
        changed_with: nodes(v.changed_with),
    }
}

fn radar_view(v: RadarView) -> pb::RadarView {
    pb::RadarView {
        unsolved_problems: nodes(v.unsolved_problems),
        decisions_without_rationale: records(v.decisions_without_rationale),
        open_questions: v.open_questions,
        stale_entities: nodes(v.stale_entities),
        alias_candidates: v.alias_candidates.into_iter().map(alias).collect(),
        dismissed: v.dismissed.iter().map(dismissal_info).collect(),
    }
}

/// A rendered graph view.
pub fn graph_view(view: RenderedView) -> pb::GraphView {
    use pb::graph_view::View;
    let view = match view {
        RenderedView::Entity(v) => View::Entity(entity_view(v)),
        RenderedView::Impact(v) => View::Impact(impact_view(v)),
        RenderedView::Topics(v) => View::Topics(topics_view(v)),
        RenderedView::Lineage(v) => View::Lineage(lineage_view(v)),
        RenderedView::Radar(v) => View::Radar(radar_view(v)),
        RenderedView::File(v) => View::File(file_view(v)),
    };
    pb::GraphView { view: Some(view) }
}
