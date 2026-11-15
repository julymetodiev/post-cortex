// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The operations behind `query_conversation_context`: graph views, typed
//! queries, the assembly alias, related content, decisions and insights,
//! entity importance and network, statistics.

use std::collections::HashMap;

use crate::Memory;
use crate::capabilities::{
    AssembleRequest, ContextQuery, ContextResponse, DEFAULT_TOKEN_BUDGET, Target,
};
use crate::graph_views::ViewRequest;
use post_cortex_core::core::context_update::EntityType;
use uuid::Uuid;

use crate::tools::Failure;
use crate::tools::handlers::Outcome;
use crate::tools::handlers::query::number;
use crate::tools::handlers::query::{param, workspace_param};
use crate::tools::reply;

type Params = HashMap<String, String>;

/// `"Failed to load session: <e>"`, as these query types have always said.
fn load_failure<E: Into<Failure> + std::fmt::Display>(e: E) -> Failure {
    let message = format!("Failed to load session: {e}");
    e.into().with_message(message)
}

/// A graph view of the session, or of the workspace of `workspace_id`.
pub async fn view(memory: &Memory, view: &str, session_id: Uuid, p: &Params) -> Outcome {
    let request =
        ViewRequest::parse(view, p).map_err(|e| Failure::invalid(e).parameter("query_type"))?;
    let target = Target::of(Some(session_id), workspace_param(p)?).map_err(Failure::from)?;
    let rendered = memory
        .views
        .view(target, request)
        .await
        .map_err(Failure::from)?;
    reply::with(format!("{view} view"), &rendered)
}

/// One record of the session whole (`record`, its `entry_id`): what a
/// search's snippet left out.
pub async fn record(memory: &Memory, session_id: Uuid, p: &Params) -> Outcome {
    let raw = p.get("entry_id").map(String::as_str).unwrap_or_default();
    let id = Uuid::parse_str(raw).map_err(|_| {
        Failure::invalid(format!("entry_id: {raw:?} is not a record's UUID")).parameter("entry_id")
    })?;
    let record = memory
        .detail_views
        .record(Target::Session(session_id), id)
        .await
        .map_err(Failure::from)?;
    let whole = crate::tools::responses::RecordWhole::from(record);
    reply::with(format!("Record {}", whole.title), &whole)
}

/// A typed query over the session.
pub async fn query(memory: &Memory, query_type: &str, session_id: Uuid, p: &Params) -> Outcome {
    let text = |name: &str| p.get(name).cloned().unwrap_or_default();
    let count = |name: &str, default: usize| -> Result<usize, Failure> {
        Ok(param(p, name, "a non-negative integer")?.unwrap_or(default))
    };
    let query = match query_type {
        "recent_changes" => ContextQuery::GetRecentChanges {
            since: since(p.get("since"))?,
        },
        "code_references" => ContextQuery::FindCodeReferences {
            file_path: text("file_path"),
        },
        "structured_summary" => ContextQuery::GetStructuredSummary,
        "decisions" => ContextQuery::GetDecisions { since: None },
        "open_questions" => ContextQuery::GetOpenQuestions,
        "related_entities" | "find_related_entities" => ContextQuery::FindRelatedEntities {
            entity_name: text("entity_name"),
        },
        "entity_context" | "get_entity_context" => ContextQuery::GetEntityContext {
            entity_name: text("entity_name"),
        },
        "all_entities" => ContextQuery::GetAllEntities { entity_type: None },
        "trace_relationships" => ContextQuery::TraceRelationships {
            from_entity: text("entity_name"),
            max_depth: count("max_depth", 3)?,
        },
        "get_entity_network" => ContextQuery::GetEntityNetwork {
            center_entity: text("entity_name"),
            max_depth: count("max_depth", 2)?,
        },
        "get_most_important_entities" => ContextQuery::GetMostImportantEntities {
            limit: count("limit", 10)?,
        },
        "get_recently_mentioned_entities" => ContextQuery::GetRecentlyMentionedEntities {
            limit: count("limit", 10)?,
        },
        "analyze_entity_importance" => ContextQuery::AnalyzeEntityImportance,
        "find_entities_by_type" => {
            const EXPECTED: &str =
                "an entity type (technology, concept, problem, solution, decision, code_component)";
            let entity_type: EntityType = param(p, "entity_type", EXPECTED)?.ok_or_else(|| {
                Failure::invalid(format!("parameter \"entity_type\": \"\" is not {EXPECTED}"))
                    .parameter("entity_type")
            })?;
            ContextQuery::FindEntitiesByType { entity_type }
        }
        "search_updates" => ContextQuery::SearchUpdates {
            query: text("query"),
        },
        other => return Err(Failure::invalid(format!("Unknown query type: {other}"))),
    };
    let response = memory
        .views
        .query(session_id, query)
        .await
        .map_err(load_failure)?;
    reply::with("Query successful", &response)
}

/// `since` as [`post_cortex_core::time::parse_time`] reads it; absent when
/// not given (the capability picks the default window).
fn since(raw: Option<&String>) -> Result<Option<chrono::DateTime<chrono::Utc>>, Failure> {
    raw.filter(|r| !r.trim().is_empty())
        .map(|r| {
            post_cortex_core::time::parse_time(r).ok_or_else(|| {
                Failure::invalid(format!("Failed to parse datetime: {r}")).parameter("since")
            })
        })
        .transpose()
}

/// The assembly of the `assemble_context` tool, over the session's records,
/// with the user card of `person`.
pub async fn assemble(
    memory: &Memory,
    session_id: Uuid,
    p: &Params,
    person: Option<Uuid>,
) -> Outcome {
    let token_budget =
        param(p, "token_budget", "a non-negative integer")?.unwrap_or(DEFAULT_TOKEN_BUDGET);
    let assembled = memory
        .assembly
        .assemble(AssembleRequest {
            query: p.get("query").cloned().unwrap_or_default(),
            target: Target::Session(session_id),
            token_budget,
            corpus: post_cortex_ports::Corpus::Records,
            person,
        })
        .await
        .map_err(|e| {
            let message = format!("Failed to assemble context: {e}");
            Failure::from(e).with_message(message)
        })?;
    reply::with(
        "Query successful",
        &ContextResponse::AssembledContext(assembled),
    )
}

/// Content related to `topic` (or `query`) in the session.
pub async fn related(memory: &Memory, session_id: Uuid, p: &Params) -> Outcome {
    use crate::tools::responses::{RelatedContent, RelatedHit};

    let topic = p
        .get("topic")
        .or_else(|| p.get("query"))
        .cloned()
        .ok_or_else(|| {
            Failure::invalid("find_related_content requires a 'topic' parameter").parameter("topic")
        })?;
    let limit = match number(p, "max_results")? {
        Some(n) => Some(n),
        None => number(p, "limit")?,
    };
    let results = memory
        .search
        .related(session_id, &topic, limit)
        .await
        .map_err(|e| {
            let message = format!("Related content search failed: {e}");
            Failure::from(e).with_message(message)
        })?;
    let mut message = format!(
        "Found {} related content items for topic: '{}'\n\n",
        results.len(),
        topic
    );
    for (idx, r) in results.iter().enumerate() {
        message.push_str(&format!(
            "{}. [{:?}] Score: {:.3}\n",
            idx + 1,
            r.content_type,
            r.combined_score
        ));
        message.push_str(&format!(
            "   Time: {}\n",
            r.timestamp.format("%Y-%m-%d %H:%M:%S")
        ));
        let content = if r.text_content.chars().count() > 500 {
            format!(
                "{}...",
                r.text_content.chars().take(500).collect::<String>()
            )
        } else {
            r.text_content.clone()
        };
        message.push_str(&format!("   Content: {content}\n\n"));
    }
    let response = RelatedContent {
        session_id: session_id.to_string(),
        topic,
        related_content: results.iter().map(RelatedHit::from).collect(),
    };
    reply::with(message, &response)
}

pub async fn key_decisions(memory: &Memory, session_id: Uuid) -> Outcome {
    let decisions = memory
        .views
        .key_decisions(session_id)
        .await
        .map_err(load_failure)?;
    reply::with(
        format!("Found {} key decisions", decisions.len()),
        &decisions,
    )
}

pub async fn key_insights(memory: &Memory, session_id: Uuid, limit: Option<usize>) -> Outcome {
    let insights = memory
        .views
        .key_insights(session_id, limit)
        .await
        .map_err(load_failure)?;
    reply::with(format!("Found {} key insights", insights.len()), &insights)
}

pub async fn importance(
    memory: &Memory,
    session_id: Uuid,
    limit: Option<usize>,
    min_importance: Option<f32>,
) -> Outcome {
    let importance = memory
        .views
        .entity_importance(session_id, limit, min_importance)
        .await
        .map_err(load_failure)?;
    let page = importance.pagination;
    reply::with(
        format!(
            "Analyzed {} entities (showing {})",
            page.total_entities, page.shown
        ),
        &importance,
    )
}

pub async fn network(
    memory: &Memory,
    session_id: Uuid,
    center: Option<&str>,
    max_entities: Option<usize>,
    max_relationships: Option<usize>,
) -> Outcome {
    let network = memory
        .views
        .entity_network(session_id, center, max_entities, max_relationships)
        .await
        .map_err(load_failure)?;
    reply::with(
        format!(
            "Network view: {} entities, {} relationships",
            network.entities.len(),
            network.relationships.len()
        ),
        &network,
    )
}

pub async fn statistics(memory: &Memory, session_id: Uuid) -> Outcome {
    let stats = memory
        .views
        .statistics(session_id)
        .await
        .map_err(load_failure)?;
    reply::with(format!("Session statistics for {session_id}"), &stats)
}

#[cfg(test)]
mod tests;
