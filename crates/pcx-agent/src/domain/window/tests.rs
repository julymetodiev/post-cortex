use super::*;

use serde_json::json;

fn turn(turn_id: &str, role: Role, blocks: Vec<Block>) -> Turn {
    Turn::new(turn_id.into(), role, None, blocks)
}

fn said(text: &str) -> Vec<Block> {
    vec![Block::Text { text: text.into() }]
}

/// A prompt, a tool call and its result, an answer: one task.
fn task(n: usize, size: usize) -> Vec<Turn> {
    vec![
        turn(&format!("p{n}"), Role::User, said(&"x".repeat(size))),
        turn(
            &format!("c{n}"),
            Role::Assistant,
            vec![Block::ToolUse {
                id: format!("t{n}"),
                name: "fs_read".into(),
                input: json!({ "path": "a" }),
            }],
        ),
        turn(
            &format!("r{n}"),
            Role::User,
            vec![Block::ToolResult {
                tool_use_id: format!("t{n}"),
                content: "y".repeat(size),
                is_error: false,
            }],
        ),
        turn(&format!("a{n}"), Role::Assistant, said("done")),
    ]
}

#[test]
fn a_request_is_about_four_characters_a_token_and_leaves_the_reserve() {
    let messages = vec![ModelMessage {
        role: Role::User,
        blocks: said(&"x".repeat(400)),
    }];
    assert_eq!(estimate(&"s".repeat(40), &messages, &[]), 110);
    // An unknown window counts as UNKNOWN_WINDOW_TOKENS.
    assert!(!over(UNKNOWN_WINDOW_TOKENS - RESERVE_TOKENS, 0));
    assert!(over(UNKNOWN_WINDOW_TOKENS - RESERVE_TOKENS + 1, 0));
    assert!(!over(100_000, 100_000 + RESERVE_TOKENS));
    assert!(over(100_001, 100_000 + RESERVE_TOKENS));
}

#[test]
fn a_compaction_cuts_before_a_prompt_and_keeps_a_tool_call_with_its_result() {
    // Three tasks of about 25k tokens each, then the newest prompt's call:
    // the cut keeps the newest task whole, never its result alone.
    let mut turns: Vec<Turn> = (1..=3).flat_map(|n| task(n, 50_000)).collect();
    turns.extend(task(4, 100).into_iter().take(3));
    let (before, kept) = to_compact(&turns, KEEP_RECENT_TOKENS).unwrap();
    let ids: Vec<&str> = kept.iter().map(|t| t.turn_id.as_str()).collect();
    assert_eq!(ids, ["p4", "c4", "r4"]);
    assert_eq!(before.len(), 12);

    // With room for more, the cut moves back to the oldest prompt that fits.
    let (_, kept) = to_compact(&turns, 40_000).unwrap();
    assert_eq!(kept[0].turn_id, "p3");
    // A conversation that fits whole still keeps something to sum up.
    let small: Vec<Turn> = (1..=2).flat_map(|n| task(n, 10)).collect();
    let (before, kept) = to_compact(&small, KEEP_RECENT_TOKENS).unwrap();
    assert_eq!((before.len(), kept[0].turn_id.as_str()), (4, "p2"));

    // Nothing before the newest prompt: nothing to compact.
    assert!(to_compact(&task(1, 10), KEEP_RECENT_TOKENS).is_none());
}

#[test]
fn a_turns_context_goes_with_its_prompt_once_and_search_reads_the_prompt() {
    assert!(context_block("  ").is_none());
    let block = context_block("Memory: we deploy with pcx-swap.").unwrap();
    let first = prompt_turn("p1".into(), None, Some(block.clone()), "Deploy it");
    assert_eq!(first.text, "Deploy it", "search reads the prompt alone");
    assert_eq!(first.blocks.len(), 2);
    assert!(matches!(&first.blocks[0], Block::Text { text } if text.starts_with(CONTEXT_HEAD)));
    let turns = vec![first];
    assert!(
        carries(&turns, &block),
        "the same context is not added again"
    );
    assert!(!carries(&turns, &context_block("Something new").unwrap()));
}

#[test]
fn a_long_tool_result_is_cut_for_the_model_saying_how_much() {
    assert_eq!(shown("short".into()), "short");
    let long = "é".repeat(MAX_TOOL_RESULT_CHARS + 10);
    let cut = shown(long);
    assert!(cut.starts_with(&"é".repeat(MAX_TOOL_RESULT_CHARS)));
    assert!(cut.ends_with("[10 more characters cut: ask for less, e.g. fewer results, a select, or a narrower range]"), "{}", &cut[cut.len() - 120..]);
}
