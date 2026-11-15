// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief's clock: the system's.

use crate::ports::Clock;

/// The system clock, in UTC.
pub struct SystemClock;

impl Clock for SystemClock {
    fn today(&self) -> String {
        chrono::Utc::now().format("%Y-%m-%d").to_string()
    }
}
