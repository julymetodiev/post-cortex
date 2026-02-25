// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

pub mod core;
pub mod daemon;
pub mod graph;
pub mod session;
pub mod storage;
pub mod summary;
pub mod tools;
pub mod workspace;

pub use core::error::{Result, SystemError};
pub use core::memory_system::ConversationMemorySystem;
pub use core::memory_system::SystemConfig;
pub use summary::{StructuredSummaryView, SummaryGenerator};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_system_initialization() {
        // Create a unique directory for this test
        let test_dir = format!(
            "./test_data_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::fs::create_dir_all(&test_dir).unwrap();

        let config = SystemConfig {
            data_directory: test_dir.clone(),
            ..Default::default()
        };

        let system = ConversationMemorySystem::new(config).await;
        assert!(system.is_ok());

        // Cleanup
        std::fs::remove_dir_all(&test_dir).unwrap();
    }
}
