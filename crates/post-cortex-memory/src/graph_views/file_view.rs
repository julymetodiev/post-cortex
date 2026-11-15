// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the records say about a source file.

use crate::context_assembly::analyze_impact;
use crate::graph_views::index::Index;
use crate::graph_views::record_files::files_of;
use crate::graph_views::types::{FileView, ImpactItem, RecordView};
use post_cortex_core::core::context_update::{EntityData, UpdateType};
use post_cortex_core::graph::entity_graph::entity_key;
use std::collections::HashSet;
use uuid::Uuid;

/// Path components, without `.` and empty ones; backslashes count as
/// separators.
fn path_components(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

/// Whether two paths name the same file: the shorter one is a
/// component-wise suffix of the longer, and has at least two components
/// unless both are bare file names.
fn same_path(a: &[&str], b: &[&str]) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    !short.is_empty() && long.ends_with(short) && (short.len() > 1 || a.len() == b.len())
}

impl Index<'_> {
    /// Records, problems, decisions, entities and dependents of the source
    /// file `path` (matched by trailing path components, so an absolute
    /// path finds records written with a repository-relative one).
    pub fn file_view(&self, path: &str, depth: usize, limit: usize) -> FileView {
        let index = self;
        let wanted = path_components(path);

        // Entities naming the file.
        let file_name = wanted.last().copied().unwrap_or_default();
        let stem = file_name
            .rsplit_once('.')
            .map_or(file_name, |(stem, _)| stem);
        let stem_ok = stem.chars().count() >= 4 && !GENERIC_STEMS.contains(&stem);
        let mut entities: Vec<&EntityData> = index
            .graph
            .entities
            .sorted()
            .into_iter()
            .map(|(_, e)| e)
            .filter(|e| {
                std::iter::once(&e.name).chain(&e.aliases).any(|n| {
                    same_path(&path_components(n), &wanted)
                        || (!file_name.is_empty() && n == file_name)
                        || (stem_ok && entity_key(n) == entity_key(stem))
                })
            })
            .collect();
        entities.sort_by(|a, b| a.name.cmp(&b.name));

        let mut ids: HashSet<Uuid> = index
            .records
            .values()
            .filter(|(_, u)| {
                files_of(u)
                    .iter()
                    .any(|f| same_path(&path_components(f), &wanted))
            })
            .map(|(_, u)| u.id)
            .collect();
        for e in &entities {
            ids.extend(
                index
                    .graph
                    .entity_mentions
                    .get(&e.name)
                    .into_iter()
                    .flatten(),
            );
        }
        let mut records: Vec<RecordView> =
            ids.into_iter().filter_map(|id| index.record(id)).collect();
        records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id)));

        let of_kind = |kinds: &[&str], current_only: bool| -> Vec<RecordView> {
            let mut out: Vec<RecordView> = records
                .iter()
                .filter(|r| kinds.contains(&r.interaction_type.as_str()))
                .filter(|r| !current_only || r.validity.is_current())
                .cloned()
                .collect();
            out.sort_by_key(|r| !r.validity.is_current());
            out.truncate(limit);
            out
        };
        let problems = of_kind(&[UpdateType::ProblemSolved.as_str()], false);
        let decisions = of_kind(
            &[
                UpdateType::DecisionMade.as_str(),
                UpdateType::RequirementAdded.as_str(),
            ],
            true,
        );

        let names: Vec<String> = entities.iter().map(|e| e.name.clone()).collect();
        let mut dependents = Vec::new();
        let mut seen: HashSet<String> = names.iter().cloned().collect();
        let mut frontier = names;
        for hop in 1..=depth {
            if frontier.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for impact in analyze_impact(&frontier, index.graph) {
                if seen.insert(impact.entity.clone()) {
                    next.push(impact.entity.clone());
                    dependents.push(ImpactItem {
                        entity_type: index
                            .graph
                            .entities
                            .get(&impact.entity)
                            .map(|e| e.entity_type.as_str().to_string()),
                        entity: impact.entity,
                        via: impact.depends_on,
                        relation_type: impact.relation_type.as_str().to_string(),
                        context: impact.context,
                        depth: hop,
                    });
                }
            }
            next.sort();
            frontier = next;
        }

        records.truncate(limit);
        FileView {
            path: path.to_string(),
            records,
            problems,
            decisions,
            entities: entities.into_iter().map(|e| index.node(e)).collect(),
            dependents,
        }
    }
}

/// File stems too common to name an entity on their own.
const GENERIC_STEMS: &[&str] = &[
    "main", "index", "init", "__init__", "test", "tests", "utils", "types", "README",
];
