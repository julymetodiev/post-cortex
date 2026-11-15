// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A turn's blocks both ways (`pcx.blocks.v1.ModelBlock`): the one mapping
//! of `AppendMessages`, `ListMessages` and the views' messages (W25).

use post_cortex_core::core::message::MessageBlock;
use post_cortex_proto::blocks::{
    ModelBlock, ThinkingBlock, ToolResultBlock, ToolUseBlock, model_block,
};
use tonic::Status;

/// A turn's block as memory keeps it.
///
/// # Errors
/// An empty block.
pub fn from_wire(block: ModelBlock) -> Result<MessageBlock, Status> {
    Ok(
        match block
            .block
            .ok_or_else(|| Status::invalid_argument("a turn's block is empty"))?
        {
            model_block::Block::Text(text) => MessageBlock::Text { text },
            model_block::Block::Summary(text) => MessageBlock::Summary { text },
            model_block::Block::End(end) => MessageBlock::End {
                stop: end.stop,
                detail: end.detail,
            },
            model_block::Block::Thinking(t) => MessageBlock::Thinking {
                text: t.text,
                signature: t.signature,
            },
            model_block::Block::ToolUse(u) => MessageBlock::ToolUse {
                id: u.id,
                name: u.name,
                input: serde_json::Value::Object(pcx_module_api::json::object_from_wire(
                    u.input.unwrap_or_default(),
                )),
            },
            model_block::Block::ToolResult(r) => MessageBlock::ToolResult {
                tool_use_id: r.tool_use_id,
                content: r.content,
                is_error: r.is_error,
            },
        },
    )
}

/// A turn's block as the wire carries it.
///
/// # Errors
/// A tool call stored with an input that is not an object.
pub fn to_wire(block: MessageBlock) -> Result<ModelBlock, Status> {
    let block = match block {
        MessageBlock::Text { text } => model_block::Block::Text(text),
        MessageBlock::Summary { text } => model_block::Block::Summary(text),
        MessageBlock::End { stop, detail } => {
            model_block::Block::End(post_cortex_proto::blocks::TurnEndBlock { stop, detail })
        }
        MessageBlock::Thinking { text, signature } => {
            model_block::Block::Thinking(ThinkingBlock { text, signature })
        }
        MessageBlock::ToolUse { id, name, input } => {
            let serde_json::Value::Object(input) = input else {
                return Err(Status::internal(format!(
                    "tool call {id} is stored with an input that is not an object"
                )));
            };
            model_block::Block::ToolUse(ToolUseBlock {
                id,
                name,
                input: Some(pcx_module_api::json::object_to_wire(input)),
            })
        }
        MessageBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => model_block::Block::ToolResult(ToolResultBlock {
            tool_use_id,
            content,
            is_error,
        }),
    };
    Ok(ModelBlock { block: Some(block) })
}
