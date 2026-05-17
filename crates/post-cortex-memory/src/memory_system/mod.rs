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

//! Conversation memory system with actor-based storage and lock-free session management.

/// Circuit-breaker pattern for graceful degradation under sustained failures.
pub mod circuit_breaker;
/// Operation types and system configuration
pub mod config;
mod embeddings;
/// Metric containers for incremental context processing and graph management
pub mod managers;
/// System-level and session-level metrics snapshots
pub mod metrics;
/// Session lifecycle management with DashMap-based caching
pub mod session_manager;
/// Actor-based storage communication layer
pub mod storage_actor;
/// Top-level conversation memory system orchestrator
pub mod system;

pub use circuit_breaker::CircuitBreaker;
pub use config::{EmbeddingConfigHolder, OperationType, SystemConfig};
pub use managers::{IncrementalContextProcessor, SimpleGraphManager};
pub use metrics::{CircuitBreakerStats, SessionManagerMetrics, SystemHealth, SystemMetrics};
pub use session_manager::SessionManager;
pub use storage_actor::{StorageActor, StorageActorHandle, StorageMessage};
pub use system::ConversationMemorySystem;
