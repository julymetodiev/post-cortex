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

//! Query-cache management and recency-bias metrics surface.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::{debug, info};
use uuid::Uuid;

use super::vectorizer::ContentVectorizer;

impl ContentVectorizer {
    /// Get query cache statistics, if caching is enabled.
    pub fn get_query_cache_stats(&self) -> Option<crate::query_cache::QueryCacheStatsSnapshot> {
        self.query_cache.as_ref().map(|cache| cache.get_stats())
    }

    /// Get query cache efficiency metrics, if caching is enabled.
    pub fn get_cache_efficiency_metrics(&self) -> Option<HashMap<String, f32>> {
        self.query_cache
            .as_ref()
            .map(|cache| cache.get_efficiency_metrics())
    }

    /// Get recency-bias performance metrics (lock-free).
    ///
    /// Returns metrics for temporal decay calculations when `recency_bias > 0`.
    /// Metrics include total time, result counts, and averages across all searches.
    ///
    /// Returns `None` if no search with recency bias has ever been performed.
    pub fn get_recency_bias_metrics(&self) -> Option<HashMap<String, f32>> {
        let calculation_count = self.recency_bias_calculation_count.load(Ordering::Relaxed);
        if calculation_count == 0 {
            return None;
        }

        let total_duration_ns = self.recency_bias_total_duration_ns.load(Ordering::Relaxed);
        let total_results = self.recency_bias_total_results.load(Ordering::Relaxed);

        let mut metrics = HashMap::new();
        metrics.insert(
            "recency_bias.total_duration_ns".to_string(),
            total_duration_ns as f32,
        );
        metrics.insert(
            "recency_bias.total_results".to_string(),
            total_results as f32,
        );
        metrics.insert(
            "recency_bias.calculation_count".to_string(),
            calculation_count as f32,
        );
        metrics.insert(
            "recency_bias.avg_duration_ns".to_string(),
            total_duration_ns as f32 / calculation_count as f32,
        );
        metrics.insert(
            "recency_bias.avg_results_per_calculation".to_string(),
            total_results as f32 / calculation_count as f32,
        );

        let avg_time_per_result = if total_results > 0 {
            total_duration_ns as f32 / total_results as f32
        } else {
            0.0
        };
        metrics.insert(
            "recency_bias.avg_time_per_result_ns".to_string(),
            avg_time_per_result,
        );

        Some(metrics)
    }

    /// Clear the query cache.
    ///
    /// # Errors
    /// Returns an error if the cache clear operation fails.
    pub async fn clear_query_cache(&self) -> Result<()> {
        if let Some(ref cache) = self.query_cache {
            cache.clear()?;
            info!("Query cache cleared successfully");
        }
        Ok(())
    }

    /// Invalidate cache entries for a specific session.
    /// More efficient than clearing the entire cache.
    ///
    /// # Errors
    /// Returns an error if the cache invalidation fails.
    pub async fn invalidate_session_cache(&self, session_id: Uuid) -> Result<()> {
        if let Some(ref cache) = self.query_cache {
            cache.invalidate_session(session_id)?;
            debug!("Invalidated cache entries for session {}", session_id);
        }
        Ok(())
    }

    /// Check whether query caching is enabled.
    pub const fn is_query_caching_enabled(&self) -> bool {
        self.query_cache.is_some()
    }
}
