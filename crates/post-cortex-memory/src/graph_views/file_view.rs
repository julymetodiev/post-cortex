// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the records say about a source file.

use crate::context_assembly::analyze_impact;
use crate::graph_views::index::Index;
use crate::graph_views::record_files::files_of;
use crate::graph_views::types::{FileView, ImpactItem, RecordView};
use post_cortex_core::core::context_update::{ContextUpdate, EntityData, EntityType, UpdateType};
use post_cortex_core::graph::entity_graph::entity_key;
use std::collections::{HashMap, HashSet};
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

/// Whether the entity `name` is named after the stem whose key is
/// `stem_key`: the stem itself (`StorageActor`), an item of it
/// (`Scope::working_in`, `veil_core::limiter::Limiter`: any `::` segment
/// before the item), or a member of a type named after it
/// (`Scope.tenant`: the owner starts upper-case).
fn named_after_stem(name: &str, stem_key: &str) -> bool {
    let mut segments: Vec<&str> = name.split("::").collect();
    segments.pop();
    entity_key(name) == stem_key
        || segments.iter().any(|s| entity_key(s) == stem_key)
        || name.split_once('.').is_some_and(|(owner, member)| {
            owner.starts_with(char::is_uppercase)
                && !member.is_empty()
                && entity_key(owner) == stem_key
        })
}

/// The entity names a record gives, resolved in `index`'s graph.
fn record_entities<'a>(
    index: &'a Index<'_>,
    u: &'a ContextUpdate,
) -> impl Iterator<Item = &'a str> {
    u.creates_entities
        .iter()
        .chain(&u.references_entities)
        .chain(u.typed_entities.iter().map(|t| &t.name))
        .filter_map(|n| index.graph.resolve(n))
}

impl Index<'_> {
    /// Records, problems, decisions, entities, dependents and the code
    /// changed with it of the source file `path` (matched by trailing path
    /// components, so an absolute path finds records written with a
    /// repository-relative one), as docs/memory.md's `file` view says.
    pub fn file_view(&self, path: &str, depth: usize, limit: usize) -> FileView {
        let index = self;
        let wanted = path_components(path);
        let file_name = wanted.last().copied().unwrap_or_default();

        // The records that name the file, each with whether it also names
        // another file of that file name; the file names of every other
        // remembered file (a path with a directory: a bare name may be
        // this one).
        let mut naming: Vec<(&ContextUpdate, bool)> = Vec::new();
        let mut other_names: HashSet<String> = HashSet::new();
        for &(_, u) in index.records.values() {
            let (mut names_it, mut names_namesake) = (false, false);
            for f in files_of(u) {
                let components = path_components(&f);
                if same_path(&components, &wanted) {
                    names_it = true;
                } else if let [_, .., name] = components.as_slice() {
                    names_namesake |= *name == file_name;
                    other_names.insert((*name).to_string());
                }
            }
            if names_it {
                naming.push((u, names_namesake));
            }
        }
        let name_shared = other_names.contains(file_name);

        // Entities named after the file: its path or file name; its stem
        // when no other remembered file has its file name, or when a record
        // that names it and no namesake names the entity.
        let stem = file_name
            .rsplit_once('.')
            .map_or(file_name, |(stem, _)| stem);
        let stem_key = entity_key(stem);
        let stem_ok = stem.chars().count() >= 4 && !GENERIC_STEMS.contains(&stem);
        let vouched: HashSet<&str> = naming
            .iter()
            .filter(|(_, names_namesake)| !names_namesake)
            .flat_map(|(u, _)| record_entities(index, u))
            .collect();
        let mut entities: Vec<&EntityData> = index
            .graph
            .entities
            .sorted()
            .into_iter()
            .map(|(_, e)| e)
            .filter(|e| {
                std::iter::once(&e.name).chain(&e.aliases).any(|n| {
                    let components = path_components(n);
                    let another_file = components
                        .last()
                        .is_some_and(|c| *c != file_name && other_names.contains(*c));
                    same_path(&components, &wanted)
                        || (!file_name.is_empty() && n == file_name)
                        || (stem_ok
                            && !another_file
                            && named_after_stem(n, &stem_key)
                            && (!name_shared || vouched.contains(e.name.as_str())))
                })
            })
            .collect();
        entities.sort_by(|a, b| a.name.cmp(&b.name));

        let mut ids: HashSet<Uuid> = naming.iter().map(|(u, _)| u.id).collect();
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

        // What depends on its entities, hop by hop; in a hop, code first.
        let code = EntityType::CodeComponent.as_str();
        let mut frontier: Vec<String> = entities.iter().map(|e| e.name.clone()).collect();
        let mut seen: HashSet<String> = frontier.iter().cloned().collect();
        let mut dependents: Vec<ImpactItem> = Vec::new();
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
        dependents.sort_by_key(|d| (d.depth, d.entity_type.as_deref() != Some(code)));

        // The code the records that name it changed with it: not its own
        // entities nor its dependents (said with more above), the most
        // often named first.
        let mut together: HashMap<&str, usize> = HashMap::new();
        for (u, _) in &naming {
            let names: HashSet<&str> = record_entities(index, u).collect();
            for name in names {
                *together.entry(name).or_default() += 1;
            }
        }
        let mut changed_with: Vec<(&EntityData, usize)> = together
            .into_iter()
            .filter(|(name, _)| !seen.contains(*name))
            .filter_map(|(name, count)| Some((index.graph.entities.get(name)?, count)))
            .filter(|(e, _)| e.entity_type == EntityType::CodeComponent)
            .collect();
        changed_with.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.name.cmp(&b.name)));
        changed_with.truncate(limit);

        records.truncate(limit);
        FileView {
            path: path.to_string(),
            records,
            problems,
            decisions,
            entities: entities.into_iter().map(|e| index.node(e)).collect(),
            dependents,
            changed_with: changed_with
                .into_iter()
                .map(|(e, _)| index.node(e))
                .collect(),
        }
    }
}

/// File stems too common to name an entity on their own.
const GENERIC_STEMS: &[&str] = &[
    "main", "index", "init", "__init__", "test", "tests", "utils", "types", "README",
];
