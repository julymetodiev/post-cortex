// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A view request by name and parameters, its parsing and rendering.

use crate::graph_views::GraphScope;
use crate::graph_views::index::Index;
use crate::graph_views::types::{
    EntityView, FileView, ImpactView, LineageView, RadarView, TopicsView,
};
use chrono::{DateTime, Utc};
use post_cortex_core::core::dismissal::RadarDismissal;
use std::collections::HashMap;

/// Default number of nodes, records or events a view returns.
pub const DEFAULT_LIMIT: usize = 100;

/// Most a caller may ask for.
pub const MAX_LIMIT: usize = 5000;

/// One view with its parameters, parsed from string pairs so every
/// transport (REST query string, MCP `parameters`, gRPC map) shares one
/// parser.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewRequest {
    /// [`GraphScope::entity_view`].
    Entity {
        /// Entity name or alias.
        name: String,
        /// Most records.
        limit: usize,
    },
    /// [`GraphScope::impact_view`].
    Impact {
        /// Entity name or alias.
        name: String,
        /// Hops.
        depth: usize,
        /// Most risk records.
        limit: usize,
    },
    /// [`GraphScope::topics_view`].
    Topics {
        /// Most topics.
        limit: usize,
        /// Entities and decisions per topic.
        per_topic: usize,
    },
    /// [`GraphScope::lineage_view`].
    Lineage {
        /// Entity name or alias.
        name: String,
        /// Hops.
        depth: usize,
    },
    /// [`GraphScope::radar_view_with`].
    Radar {
        /// Most items per list.
        limit: usize,
        /// Keep dismissed items in the lists.
        include_dismissed: bool,
    },
    /// [`GraphScope::file_view`].
    File {
        /// Source file path.
        path: String,
        /// Hops of impact analysis.
        depth: usize,
        /// Most records per list.
        limit: usize,
    },
}

/// View names [`ViewRequest::parse`] accepts.
pub const VIEW_NAMES: &[&str] = &["entity", "impact", "topics", "lineage", "radar", "file"];

impl ViewRequest {
    /// The entity view of `name`: at most `limit` records (none: 100).
    ///
    /// # Errors
    /// `name` is empty.
    pub fn entity(name: &str, limit: Option<usize>) -> Result<Self, String> {
        Ok(Self::Entity {
            name: entity_name("entity", name)?,
            limit: bounded(limit, DEFAULT_LIMIT, MAX_LIMIT),
        })
    }

    /// The impact view of `name`: `depth` hops (none: 3, at most 10), at
    /// most `limit` risk records (none: 100).
    ///
    /// # Errors
    /// `name` is empty.
    pub fn impact(name: &str, depth: Option<usize>, limit: Option<usize>) -> Result<Self, String> {
        Ok(Self::Impact {
            name: entity_name("impact", name)?,
            depth: bounded(depth, 3, 10),
            limit: bounded(limit, DEFAULT_LIMIT, MAX_LIMIT),
        })
    }

    /// The topics view: at most `limit` topics (none: 100), `per_topic`
    /// entities and decisions each (none: 5, at most 50).
    #[must_use]
    pub fn topics(limit: Option<usize>, per_topic: Option<usize>) -> Self {
        Self::Topics {
            limit: bounded(limit, DEFAULT_LIMIT, MAX_LIMIT),
            per_topic: bounded(per_topic, 5, 50),
        }
    }

    /// The lineage view of `name`: `depth` hops (none: 3, at most 10).
    ///
    /// # Errors
    /// `name` is empty.
    pub fn lineage(name: &str, depth: Option<usize>) -> Result<Self, String> {
        Ok(Self::Lineage {
            name: entity_name("lineage", name)?,
            depth: bounded(depth, 3, 10),
        })
    }

    /// The radar view: at most `limit` items per list (none: 100), the
    /// dismissed ones kept with `include_dismissed`.
    #[must_use]
    pub fn radar(limit: Option<usize>, include_dismissed: bool) -> Self {
        Self::Radar {
            limit: bounded(limit, DEFAULT_LIMIT, MAX_LIMIT),
            include_dismissed,
        }
    }

    /// The file view of `path`: `depth` hops of impact analysis (none: 2,
    /// at most 10), at most `limit` records per list (none: 20).
    ///
    /// # Errors
    /// `path` is empty.
    pub fn file(path: &str, depth: Option<usize>, limit: Option<usize>) -> Result<Self, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("view 'file' needs a 'path' parameter".to_string());
        }
        Ok(Self::File {
            path: path.to_string(),
            depth: bounded(depth, 2, 10),
            limit: bounded(limit, 20, MAX_LIMIT),
        })
    }

    /// Parse view `view` from `params`: `limit`, `depth`, `per_topic`,
    /// `entity` (or `entity_name`) for the entity views, `path` for `file`
    /// and `include_dismissed` (true/false) for `radar`.
    pub fn parse(view: &str, params: &HashMap<String, String>) -> Result<Self, String> {
        let get = |k: &str| params.get(k).map(|v| v.trim()).filter(|v| !v.is_empty());
        let number = |k: &str| -> Result<Option<usize>, String> {
            get(k)
                .map(|v| {
                    v.parse::<usize>()
                        .map_err(|_| format!("{k} must be a positive integer, got '{v}'"))
                })
                .transpose()
        };
        let name = get("entity").or_else(|| get("entity_name")).unwrap_or("");
        match view {
            "entity" => Self::entity(name, number("limit")?),
            "impact" => Self::impact(name, number("depth")?, number("limit")?),
            "topics" => Ok(Self::topics(number("limit")?, number("per_topic")?)),
            "lineage" => Self::lineage(name, number("depth")?),
            "radar" => {
                let include_dismissed = match get("include_dismissed") {
                    None => false,
                    Some(v) => match v.to_ascii_lowercase().as_str() {
                        "true" | "1" | "yes" => true,
                        "false" | "0" | "no" => false,
                        _ => {
                            return Err(format!(
                                "include_dismissed must be true or false, got '{v}'"
                            ));
                        }
                    },
                };
                Ok(Self::radar(number("limit")?, include_dismissed))
            }
            "file" => Self::file(
                get("path").unwrap_or(""),
                number("depth")?,
                number("limit")?,
            ),
            other => Err(format!(
                "unknown view '{other}'; expected one of {}",
                VIEW_NAMES.join(", ")
            )),
        }
    }
}

/// `n` clamped to `1..=max`; none: `default`.
fn bounded(n: Option<usize>, default: usize, max: usize) -> usize {
    n.map_or(default, |n| n.clamp(1, max))
}

/// The entity `name` the view `view` is of.
fn entity_name(view: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("view '{view}' needs an 'entity' parameter"));
    }
    Ok(name.to_string())
}

/// Why a view could not be rendered.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ViewError {
    /// The entity is not in the scope.
    #[error("entity '{0}' not found in scope")]
    EntityNotFound(String),
}

/// A rendered view: serialises as the view itself (untagged), the shape
/// every transport sends.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum RenderedView {
    /// [`ViewRequest::Entity`].
    Entity(EntityView),
    /// [`ViewRequest::Impact`].
    Impact(ImpactView),
    /// [`ViewRequest::Topics`].
    Topics(TopicsView),
    /// [`ViewRequest::Lineage`].
    Lineage(LineageView),
    /// [`ViewRequest::Radar`].
    Radar(RadarView),
    /// [`ViewRequest::File`].
    File(FileView),
}

impl GraphScope {
    /// Render `request` as of `now`, leaving out the radar items
    /// `dismissals` name (see [`Self::radar_view_with`]); other views
    /// ignore them.
    pub fn render_with(
        &self,
        request: &ViewRequest,
        dismissals: &[RadarDismissal],
        now: DateTime<Utc>,
    ) -> Result<RenderedView, ViewError> {
        let missing = |name: &str| ViewError::EntityNotFound(name.to_string());
        let index = Index::of(self);
        Ok(match request {
            ViewRequest::Entity { name, limit } => RenderedView::Entity(
                index
                    .entity_view(name, *limit)
                    .ok_or_else(|| missing(name))?,
            ),
            ViewRequest::Impact { name, depth, limit } => RenderedView::Impact(
                index
                    .impact_view(name, *depth, *limit)
                    .ok_or_else(|| missing(name))?,
            ),
            ViewRequest::Topics { limit, per_topic } => {
                RenderedView::Topics(index.topics_view(*limit, *per_topic))
            }
            ViewRequest::Lineage { name, depth } => RenderedView::Lineage(
                index
                    .lineage_view(name, *depth)
                    .ok_or_else(|| missing(name))?,
            ),
            ViewRequest::Radar {
                limit,
                include_dismissed,
            } => RenderedView::Radar(index.radar_view_with(
                now,
                *limit,
                dismissals,
                *include_dismissed,
            )),
            ViewRequest::File { path, depth, limit } => {
                RenderedView::File(index.file_view(path, *depth, *limit))
            }
        })
    }
}
