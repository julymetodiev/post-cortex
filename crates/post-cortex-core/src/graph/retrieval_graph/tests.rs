// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::core::context_update::RelationType;

fn record(
    id: usize,
    text: &str,
    entities: Vec<&'static str>,
) -> (usize, String, Vec<&'static str>) {
    (id, text.to_string(), entities)
}

fn graph(
    rows: &[(usize, String, Vec<&'static str>)],
    relations: &[EntityRelationship],
) -> RetrievalGraph<usize> {
    RetrievalGraph::build(
        rows.iter().map(|(id, text, entities)| GraphRecord {
            id: *id,
            terms: terms(text),
            entities: entities.iter().map(|e| e.to_string()).collect(),
        }),
        relations,
    )
}

/// Filler so that no real node is above the 5% df cut-off, while common
/// words ("the", "about") are, as in real text.
fn filler(rows: &mut Vec<(usize, String, Vec<&'static str>)>) {
    rows.extend((100..160).map(|i| {
        record(
            i,
            &format!("the filler record number {i} about the weather"),
            vec![],
        )
    }));
}

#[test]
fn keys_round_trip_sorted_and_deduplicated() {
    let joined = join_keys(["new york", "adopt", "adopt", ""]);
    assert_eq!(joined, "adopt\nnew york");
    assert_eq!(
        split_keys(&joined).collect::<Vec<_>>(),
        ["adopt", "new york"]
    );
    assert_eq!(split_keys("").count(), 0);
}

#[test]
fn terms_are_stemmed_words_and_names() {
    let t = terms("Caroline researched adoption agencies in New York with Melanie.");
    for want in [
        "caroline", "new york", "melanie", "research", "adopt", "agenc",
    ] {
        assert!(t.contains(want), "{want} missing from {t:?}");
    }
    assert!(
        !t.contains("in"),
        "words under three characters are skipped"
    );
}

#[test]
fn terms_work_in_any_script() {
    let t = terms("Přišel jsem do Prahy, viděl jsem kočku.");
    assert!(t.contains("prahy") && t.contains("kočku"), "{t:?}");
    let t = terms("Решихме да ползваме SurrealDB за базата.");
    assert!(
        t.contains("решихме") && t.contains("базата") && t.contains("surrealdb"),
        "{t:?}"
    );
}

/// A record sharing no term with the query is reached through a record
/// that does (the multi-hop case).
#[test]
fn reaches_second_hop_records_through_terms() {
    let mut rows = vec![
        record(0, "Caroline adopted a puppy called Biscuit", vec![]),
        record(1, "Biscuit chewed the sofa yesterday", vec![]),
        record(2, "Melanie painted a sunrise over the lake", vec![]),
    ];
    filler(&mut rows);
    let g = graph(&rows, &[]);
    let ranked = g.rank("What did Caroline's puppy chew?", 10);
    assert_eq!(ranked.first(), Some(&0));
    assert!(ranked.contains(&1), "{ranked:?}");
    assert!(!ranked.contains(&2), "{ranked:?}");
    assert!(g.rank("unrelated zebracorn", 10).is_empty());
}

/// Agent entities and their relations connect records whose text does
/// not overlap at all.
#[test]
fn agent_entities_and_relations_join_records() {
    let mut rows = vec![
        record(0, "switched the storage layer", vec!["SurrealDB"]),
        record(
            1,
            "index rebuild blocks startup for minutes",
            vec!["HNSW index"],
        ),
        record(2, "unrelated note about lunch", vec![]),
    ];
    filler(&mut rows);
    let relations = [EntityRelationship {
        from_entity: "HNSW index".into(),
        to_entity: "SurrealDB".into(),
        relation_type: RelationType::DependsOn,
        context: String::new(),
    }];
    let ranked = graph(&rows, &relations).rank("what do we know about surrealdb?", 10);
    assert_eq!(ranked.first(), Some(&0), "{ranked:?}");
    assert!(ranked.contains(&1), "related entity's record: {ranked:?}");
    assert!(!ranked.contains(&2));
    // Without the relation the second record is unreachable.
    let ranked = graph(&rows, &[]).rank("what do we know about surrealdb?", 10);
    assert!(!ranked.contains(&1), "{ranked:?}");
}

/// Entities seed only when the query names them as whole words: "Go" is
/// not in "a good day" (it was, as a substring).
#[test]
fn entity_seeds_are_whole_words() {
    let mut rows = vec![record(0, "picked the language", vec!["Go"])];
    filler(&mut rows);
    let g = graph(&rows, &[]);
    assert!(g.rank("zebracorn had a good day", 10).is_empty());
    assert_eq!(g.rank("why Go?", 10).first(), Some(&0));
    assert_eq!(g.rank("Go's runtime", 10).first(), Some(&0));
}

/// A relation joins the records of its entities however they are spelt:
/// nodes are keyed by the resolution key (they were keyed lowercase only,
/// so "SurrealDB" and "surreal-db" were two nodes and the edge was lost).
#[test]
fn relations_join_spellings_of_one_entity() {
    let mut rows = vec![
        record(0, "switched the storage layer", vec!["surreal-db"]),
        record(1, "index rebuild blocks startup", vec!["HNSW index"]),
    ];
    filler(&mut rows);
    let relations = [EntityRelationship {
        from_entity: "HNSW Index".into(),
        to_entity: "SurrealDB".into(),
        relation_type: RelationType::DependsOn,
        context: String::new(),
    }];
    let ranked = graph(&rows, &relations).rank("what about surreal db?", 10);
    assert_eq!(ranked.first(), Some(&0), "{ranked:?}");
    assert!(ranked.contains(&1), "{ranked:?}");
}
