// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::{build_entity_description, chunk_text};

/// Another mention leaves an entity's description, and so its vector,
/// as it was; a new alias changes it.
#[test]
fn entity_description_ignores_mention_counts() {
    use post_cortex_core::core::context_update::EntityType;
    use post_cortex_core::session::active_session::ActiveSession;

    let mut session = ActiveSession::new(uuid::Uuid::new_v4(), None, None, chrono::Utc::now());
    let now = chrono::Utc::now();
    let graph = session.entity_graph_mut();
    graph.add_or_update_entity("SurrealDB".into(), EntityType::Technology, now, "");
    let describe = |s: &ActiveSession| {
        build_entity_description(s, "SurrealDB", &s.entity_graph().entities["SurrealDB"])
    };
    let before = describe(&session);
    let graph = session.entity_graph_mut();
    graph.add_or_update_entity("SurrealDB".into(), EntityType::Technology, now, "");
    assert_eq!(describe(&session), before);
    let graph = session.entity_graph_mut();
    graph.add_or_update_entity("surreal-db".into(), EntityType::Technology, now, "");
    assert!(describe(&session).contains("Also: surreal-db"));
}

#[test]
fn short_text_is_one_chunk() {
    assert_eq!(chunk_text("a few words", 1000, 200), ["a few words"]);
}

#[test]
fn long_text_is_cut_at_words_with_overlap() {
    let text: String = (0..400).map(|i| format!("word{i} ")).collect();
    let chunks = chunk_text(&text, 1000, 200);
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        assert!(chunk.chars().count() <= 1000);
        assert!(chunk.starts_with("word") && chunk.ends_with(|c: char| c.is_ascii_digit()));
    }
    // Consecutive chunks overlap, and together they cover the text.
    for pair in chunks.windows(2) {
        let last = pair[0].split(' ').next_back().unwrap();
        assert!(pair[1].contains(last), "no overlap at {last}");
    }
    assert!(chunks.last().unwrap().ends_with("word399"));
}

#[test]
fn multibyte_text_is_cut_on_char_boundaries() {
    let text = "дума ".repeat(500);
    let chunks = chunk_text(&text, 300, 50);
    assert!(chunks.len() > 1);
    assert!(chunks.iter().all(|c| c.chars().count() <= 300));
}
