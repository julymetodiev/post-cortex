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

use std::collections::HashMap;

use crate::content_vectorizer::vectorizer::ContentVectorizer;

impl ContentVectorizer {
    /// Get query cache statistics, if caching is enabled.
    pub fn get_query_cache_stats(&self) -> Option<crate::query_cache::QueryCacheStatsSnapshot> {
        self.query_cache().map(|cache| cache.get_stats())
    }

    /// Get query cache efficiency metrics, if caching is enabled.
    pub fn get_cache_efficiency_metrics(&self) -> Option<HashMap<String, f32>> {
        self.query_cache()
            .map(|cache| cache.get_efficiency_metrics())
    }

    /// Get recency-bias performance metrics (lock-free).
    ///
    /// Returns metrics for temporal decay calculations when `recency_bias > 0`.
    /// Metrics include total time, result counts, and averages across all searches.
    ///
    /// Returns `None` if no search with recency bias has ever been performed.
    pub fn get_recency_bias_metrics(&self) -> Option<HashMap<String, f32>> {
        let (total_duration_ns, total_results, calculation_count) = self.recency_totals();
        if calculation_count == 0 {
            return None;
        }

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
    pub fn clear_query_cache(&self) {
        if let Some(cache) = self.query_cache() {
            cache.clear();
        }
    }

    /// Check whether query caching is enabled.
    pub fn is_query_caching_enabled(&self) -> bool {
        self.query_cache().is_some()
    }
}
