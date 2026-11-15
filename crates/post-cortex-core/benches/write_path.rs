//! Cost of one write to a session of `n` records while the previous
//! version stays alive (as it does in the memory crate's session cache,
//! which publishes every write as a new snapshot).
#![allow(missing_docs)]

use chrono::{Duration, TimeZone, Utc};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use post_cortex_core::core::context_update::{
    CodeReference, ContextUpdate, EntityRelationship, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;
use std::hint::black_box;
use uuid::Uuid;

const TYPES: [UpdateType; 6] = [
    UpdateType::QuestionAnswered,
    UpdateType::ProblemSolved,
    UpdateType::CodeChanged,
    UpdateType::DecisionMade,
    UpdateType::ConceptDefined,
    UpdateType::RequirementAdded,
];

/// Record `i`: ~1 KB of text, three entities (one shared hub), two
/// relations, a code reference on every third record.
fn record(i: usize) -> ContextUpdate {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let a = format!("component{}", i % 400);
    let b = format!("concept{}", i % 150);
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: base + Duration::seconds(i as i64),
        update_type: TYPES[i % TYPES.len()],
        content: UpdateContent {
            title: format!("record {i} about {a} and {b}"),
            description: format!("{a} depends on {b}. ").repeat(40),
            details: vec![format!("detail {i}")],
            examples: vec![],
            implications: vec![],
        },
        related_code: i.is_multiple_of(3).then(|| CodeReference {
            file_path: format!("src/file{}.rs", i % 50),
            start_line: 1,
            end_line: 10,
            code_snippet: "fn f() {}".into(),
            commit_hash: None,
            branch: None,
            change_description: format!("change {i}"),
        }),
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec![a.clone(), b.clone(), "hub".into()],
        creates_relationships: vec![
            EntityRelationship {
                from_entity: a.clone(),
                to_entity: b.clone(),
                relation_type: RelationType::DependsOn,
                context: "bench".into(),
            },
            EntityRelationship {
                from_entity: a,
                to_entity: "hub".into(),
                relation_type: RelationType::RelatedTo,
                context: "bench".into(),
            },
        ],
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    }
}

/// The write the memory crate performs for one record: the record and its
/// graph merge on a copy of the current version.
fn write(session: &mut ActiveSession, update: ContextUpdate) {
    session.apply_record(update);
}

fn session_of(n: usize) -> ActiveSession {
    let mut session = ActiveSession::new(
        Uuid::new_v4(),
        Some("bench".into()),
        None,
        chrono::Utc::now(),
    );
    for i in 0..n {
        write(&mut session, record(i));
    }
    session
}

fn bench_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("session_write");
    for &n in &[100usize, 1_000, 5_000] {
        let current = session_of(n);
        group.bench_with_input(BenchmarkId::new("append_one", n), &n, |b, &n| {
            b.iter_batched(
                || record(n),
                |update| {
                    // `current` stays alive, like the published snapshot.
                    let mut next = current.clone();
                    write(&mut next, update);
                    black_box(next)
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(benches, bench_write);
criterion_main!(benches);
