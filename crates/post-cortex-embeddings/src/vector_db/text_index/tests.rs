// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

fn index() -> TextIndex {
    let mut index = TextIndex::default();
    index.add(1, "Decided to keep RocksDB for the local daemon");
    index.add(
        2,
        "SurrealDB HNSW index and RocksDB column families, RocksDB tuning",
    );
    index.add(3, "Lunch plans for Friday");
    index.add(4, "Решихме да ползваме базата SurrealDB");
    index
}

/// Slots by score, best first.
fn search(index: &TextIndex, query: &str) -> Vec<u32> {
    let terms = index.query_terms(query);
    let mut ranked: Vec<(u32, f32)> = index.scores(&terms, |_| true).into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.into_iter().map(|r| r.0).collect()
}

#[test]
fn ranks_by_bm25_and_filters() {
    let index = index();
    let ranked = search(&index, "rocksdb tuning");
    assert_eq!(ranked.first(), Some(&2), "{ranked:?}");
    assert!(ranked.contains(&1) && !ranked.contains(&3));
    let terms = index.query_terms("rocksdb");
    assert_eq!(index.scores(&terms, |slot| slot == 1).len(), 1);
    assert_eq!(search(&index, "базата"), [4]);
}

/// Words on most records are dropped from queries once the memory is
/// big enough for frequencies to mean something, in any language.
#[test]
fn common_words_are_dropped_by_frequency() {
    let mut index = TextIndex::default();
    for i in 0..40 {
        index.add(i, &format!("jsem byl v práci číslo {i}"));
    }
    index.add(40, "jsem viděl kočku");
    assert_eq!(index.query_terms("Viděl jsem kočku?"), ["kočku", "viděl"]);

    let mut small = TextIndex::default();
    small.add(1, "jsem tady");
    small.add(2, "jsem tam");
    assert_eq!(small.query_terms("jsem tady"), ["jsem", "tady"]);
}

/// In a memory that is mostly English, a Bulgarian query's common words
/// are still dropped: a word's share is of the records in its script, so
/// "и" and "са" do not match every Bulgarian record and push the English
/// record a query means below them (owner, 2026-11-07).
#[test]
fn common_words_are_dropped_within_their_script() {
    let mut index = TextIndex::default();
    for i in 0..100 {
        index.add(i, &format!("the record number {i} of the memory"));
    }
    for i in 100..112 {
        index.add(i, &format!("това е запис {i} и те са на български"));
    }
    index.add(112, "сайтът е готов");
    assert_eq!(
        index.query_terms("това и сайтът са лицето"),
        ["лицето", "сайтът"]
    );
    // Removed records leave their script's count.
    for i in 100..112 {
        index.remove(i, &format!("това е запис {i} и те са на български"));
    }
    assert_eq!(index.scripts.get(&Script::Cyrillic), Some(&1));
    // English is pruned among all its records, as before.
    assert_eq!(index.query_terms("the memory of sites"), ["sites"]);
}

#[test]
fn removed_slots_are_gone_and_reusable() {
    let mut index = index();
    index.remove(
        2,
        "SurrealDB HNSW index and RocksDB column families, RocksDB tuning",
    );
    assert_eq!(search(&index, "rocksdb"), [1]);
    index.add(2, "compaction of RocksDB");
    assert_eq!(search(&index, "compaction"), [2]);
    assert!(!search(&index, "surrealdb").contains(&2));
}
