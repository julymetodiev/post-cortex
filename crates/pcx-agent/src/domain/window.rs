// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The model's window (ARCHITECTURE section 7, the agent; owner
//! 2026-10-25, after Axon's): how big a request is, when a conversation is
//! compacted before the model refuses it, and where it is cut; and the
//! context a turn carries (what memory recalls, what the modules add) as a
//! block of its prompt, so the request's prefix stays the same from turn
//! to turn and the provider's cache serves it. Pure.

use pcx_module_api::model::{Block, ModelMessage, ModelTool, Role};

use crate::domain::{Turn, chain, messages};

/// The tokens a request leaves free for the answer: a conversation whose
/// request needs more of the window is compacted first.
pub const RESERVE_TOKENS: u64 = 16_384;

/// The window counted for a model whose provider does not know it (0):
/// docs/modules/agent.md, choosing the models.
pub const UNKNOWN_WINDOW_TOKENS: u64 = 131_072;

/// The newest tokens a compaction keeps as they are, about.
pub const KEEP_RECENT_TOKENS: u64 = 20_000;

/// How many model calls of a turn that calls tools a due compaction waits.
pub const MAX_COMPACTION_DEFERRALS: u32 = 2;

/// The most characters of a tool's result the model is shown (Axon's
/// 50 KB a tool's output); a longer one is cut, saying so.
pub const MAX_TOOL_RESULT_CHARS: usize = 50 * 1024;

/// What the model is shown of a tool's result `content`: all of it, or its
/// start and how much was cut, with how to ask for less.
#[must_use]
pub fn shown(content: String) -> String {
    let total = content.chars().count();
    if total <= MAX_TOOL_RESULT_CHARS {
        return content;
    }
    format!(
        "{}\n[{} more characters cut: ask for less, e.g. fewer results, a select, or a narrower range]",
        pcx_module_api::text::clip(&content, MAX_TOOL_RESULT_CHARS),
        total - MAX_TOOL_RESULT_CHARS
    )
}

/// What opens a turn's context block.
pub const CONTEXT_HEAD: &str = "Context for this turn, from memory and the modules:";

/// About how many tokens `text` is (four characters a token).
fn tokens(text: &str) -> u64 {
    u64::try_from(text.chars().count()).map_or(u64::MAX, |n| n.div_ceil(4))
}

fn block_tokens(block: &Block) -> u64 {
    match block {
        Block::Text { text } | Block::Summary { text } => tokens(text),
        Block::Thinking { text, .. } => tokens(text),
        Block::ToolUse { name, input, .. } => tokens(name) + tokens(&input.to_string()),
        Block::ToolResult { content, .. } => tokens(content),
        // Never sent to a model.
        Block::End { .. } => 0,
    }
}

fn message_tokens(messages: &[ModelMessage]) -> u64 {
    messages
        .iter()
        .flat_map(|m| &m.blocks)
        .map(block_tokens)
        .sum()
}

/// About how many tokens a request of `system`, `messages` and `tools` is.
#[must_use]
pub fn estimate(system: &str, messages: &[ModelMessage], tools: &[ModelTool]) -> u64 {
    let tools: u64 = tools
        .iter()
        .map(|t| {
            tokens(&t.name)
                + tokens(&t.description)
                + tokens(&serde_json::Value::Object(t.input_schema.clone()).to_string())
        })
        .sum();
    tokens(system) + message_tokens(messages) + tools
}

/// Whether a request of about `needed` tokens leaves too little of a
/// `window`-token model for its answer; an unknown window (0) counts as
/// [`UNKNOWN_WINDOW_TOKENS`].
#[must_use]
pub fn over(needed: u64, window: u64) -> bool {
    let window = if window == 0 {
        UNKNOWN_WINDOW_TOKENS
    } else {
        window
    };
    needed.saturating_add(RESERVE_TOKENS) > window
}

/// A prompt of a person: a user turn that answers no tool and is no
/// summary. A compaction cuts only before one, so a tool's call and its
/// result stay together.
fn prompt(turn: &Turn) -> bool {
    turn.role == Role::User
        && !turn.summarizes()
        && !turn
            .blocks
            .iter()
            .any(|b| matches!(b, Block::ToolResult { .. }))
}

/// What a compaction sums up and what it keeps of the chain: the messages
/// before the cut, and the turns from it on, as they are. The cut is
/// before the oldest prompt but the first whose turns from it on are at
/// most `keep` tokens, else before the newest prompt; none when nothing
/// comes before that.
#[must_use]
pub fn to_compact(turns: &[Turn], keep: u64) -> Option<(Vec<ModelMessage>, Vec<&Turn>)> {
    let chain = chain(turns);
    let mut kept = 0u64;
    let (mut newest, mut oldest) = (None, None);
    for (i, turn) in chain.iter().enumerate().rev() {
        kept = kept.saturating_add(message_tokens(&messages(std::slice::from_ref(turn))));
        if !prompt(turn) || i == 0 {
            continue;
        }
        newest = newest.or(Some(i));
        if kept > keep {
            break;
        }
        oldest = Some(i);
    }
    let cut = oldest.or(newest)?;
    let (before, after) = chain.split_at(cut);
    Some((messages(before), after.to_vec()))
}

/// The context block of a turn of `context`; none when it is empty.
#[must_use]
pub fn context_block(context: &str) -> Option<Block> {
    let context = context.trim();
    (!context.is_empty()).then(|| Block::Text {
        text: format!("{CONTEXT_HEAD}\n{context}"),
    })
}

/// Whether the conversation sent from `turns` already carries `block`: the
/// same context is not added again.
#[must_use]
pub fn carries(turns: &[Turn], block: &Block) -> bool {
    chain(turns)
        .iter()
        .any(|t| t.blocks.iter().any(|b| b == block))
}

/// A person's prompt as a turn: its context block (when it has one), then
/// the prompt; its text (what search reads) the prompt alone.
#[must_use]
pub fn prompt_turn(
    turn_id: String,
    parent: Option<String>,
    context: Option<Block>,
    prompt: &str,
) -> Turn {
    let mut blocks: Vec<Block> = context.into_iter().collect();
    blocks.push(Block::Text {
        text: prompt.to_string(),
    });
    Turn {
        turn_id,
        role: Role::User,
        text: prompt.to_string(),
        parent,
        blocks,
        author: None,
    }
}

#[cfg(test)]
mod tests;
