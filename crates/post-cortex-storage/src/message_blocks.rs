// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A message's blocks as both backends store them: JSON text (a tool's
//! input is any JSON, which bincode cannot read back), none for a turn
//! without blocks.

use post_cortex_core::core::message::MessageBlock;

use crate::port_error::Result;

/// `blocks` as stored; none when there are none.
pub fn encode(turn_id: &str, blocks: &[MessageBlock]) -> Result<Option<String>> {
    if blocks.is_empty() {
        return Ok(None);
    }
    let text = serde_json::to_string(blocks).map_err(|e| {
        post_cortex_ports::error::corrupt("message", turn_id, format!("blocks: {e}"))
    })?;
    Ok(Some(text))
}

/// The blocks stored as `stored` (none: none).
pub fn decode(turn_id: &str, stored: Option<&str>) -> Result<Vec<MessageBlock>> {
    match stored {
        None => Ok(Vec::new()),
        Some(text) => Ok(serde_json::from_str(text).map_err(|e| {
            post_cortex_ports::error::corrupt("message", turn_id, format!("blocks: {e}"))
        })?),
    }
}
