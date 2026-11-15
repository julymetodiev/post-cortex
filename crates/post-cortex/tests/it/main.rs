// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The integration tests of post-cortex, in one binary: one link of the
//! whole stack instead of one per file.

mod common;
mod integration_admin_tasks;
mod integration_e2e_surrealdb;
mod integration_unified_search;
mod integration_workspace;
mod property_vector_db;
mod test_direct_encoding;
mod test_embedding_similarity;
mod test_mcp_format;
mod test_recency_bias;
mod test_recency_bias_cache;
