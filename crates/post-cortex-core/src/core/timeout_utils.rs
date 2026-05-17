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
use std::time::Duration;
use thiserror::Error;
use tokio::time::timeout;
use tracing::{debug, warn};

/// Default timeout for MCP operations (120 seconds / 2 minutes)
/// Increased to handle large text processing (entity extraction, vectorization)
pub const DEFAULT_MCP_TIMEOUT: Duration = Duration::from_secs(120);

/// Default timeout for storage operations (10 seconds)
pub const DEFAULT_STORAGE_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for entity processing (5 seconds)
pub const DEFAULT_ENTITY_PROCESSING_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout error for operations that exceed their time limit
#[derive(Error, Debug)]
pub enum TimeoutError {
    #[error("Operation timed out after {duration:?}")]
    Timeout { duration: Duration },
}

/// Execute an async operation with a timeout wrapper
pub async fn with_timeout<T>(
    operation: impl std::future::Future<Output = T>,
    duration: Duration,
) -> Result<T, TimeoutError> {
    debug!("Starting operation with timeout: {:?}", duration);

    match timeout(duration, operation).await {
        Ok(result) => {
            debug!("Operation completed successfully within timeout");
            Ok(result)
        }
        Err(_) => {
            warn!("Operation timed out after {:?}", duration);
            Err(TimeoutError::Timeout { duration })
        }
    }
}

/// Execute an MCP operation with default timeout
pub async fn with_mcp_timeout<T>(
    operation: impl std::future::Future<Output = T>,
) -> Result<T, TimeoutError> {
    with_timeout(operation, DEFAULT_MCP_TIMEOUT).await
}

/// Execute a storage operation with default timeout
pub async fn with_storage_timeout<T>(
    operation: impl std::future::Future<Output = T>,
) -> Result<T, TimeoutError> {
    with_timeout(operation, DEFAULT_STORAGE_TIMEOUT).await
}

/// Execute entity processing with default timeout
pub async fn with_entity_timeout<T>(
    operation: impl std::future::Future<Output = T>,
) -> Result<T, TimeoutError> {
    with_timeout(operation, DEFAULT_ENTITY_PROCESSING_TIMEOUT).await
}

/// Retry an operation with exponential backoff and timeout
pub async fn with_retry_timeout<T, E>(
    mut operation: impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>>>>,
    max_retries: u32,
    initial_delay: Duration,
    timeout_duration: Duration,
) -> Result<T, TimeoutError>
where
    E: std::fmt::Debug,
{
    let mut delay = initial_delay;

    for attempt in 1..=max_retries {
        debug!(
            "Attempt {}/{} with timeout {:?}",
            attempt, max_retries, timeout_duration
        );

        let result = with_timeout(operation(), timeout_duration).await;

        match result {
            Ok(Ok(value)) => {
                debug!("Operation succeeded on attempt {}", attempt);
                return Ok(value);
            }
            Ok(Err(e)) => {
                warn!("Operation failed on attempt {}: {:?}", attempt, e);
                if attempt < max_retries {
                    debug!("Retrying in {:?}", delay);
                    tokio::time::sleep(delay).await;
                    delay = delay.saturating_mul(2); // Exponential backoff
                }
            }
            Err(timeout_error) => {
                warn!(
                    "Operation timed out on attempt {}: {:?}",
                    attempt, timeout_error
                );
                if attempt == max_retries {
                    return Err(timeout_error);
                }
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
        }
    }

    Err(TimeoutError::Timeout {
        duration: timeout_duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_successful_operation() {
        let result = with_timeout(async { 42 }, Duration::from_secs(1)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_timeout_operation() {
        let result = with_timeout(
            async {
                sleep(Duration::from_secs(2)).await;
                42
            },
            Duration::from_millis(100),
        )
        .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TimeoutError::Timeout { .. }));
    }

    #[tokio::test]
    async fn test_mcp_timeout() {
        let result = with_mcp_timeout(async { "success" }).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
    }

    #[tokio::test]
    async fn test_retry_success() {
        let mut attempt_count = 0;
        let result = with_retry_timeout(
            || {
                attempt_count += 1;
                Box::pin(async move {
                    if attempt_count < 3 {
                        Err("failed")
                    } else {
                        Ok("success")
                    }
                })
            },
            5,
            Duration::from_millis(10),
            Duration::from_secs(1),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(attempt_count, 3);
    }
}
