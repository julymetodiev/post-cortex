//! Scoring strategies for semantic search results
//!
//! This module provides traits and implementations for adjusting search result scores
//! based on various factors (temporal decay, popularity, quality, etc.)
//!
//! # Overview
//!
//! The [`ScoreAdjuster`] trait allows you to compose multiple scoring strategies
//! without modifying the core search logic.

use crate::core::vector_db::VectorMetadata;
use chrono::{DateTime, Utc};

/// Trait for adjusting search result scores based on various factors
///
/// This trait enables the Strategy pattern for scoring adjustments, allowing you
/// to compose multiple scoring strategies (temporal decay, popularity boosts, quality
/// weights, etc.) without modifying the core search logic.
///
/// # Implementing ScoreAdjuster
///
/// To create a custom adjuster:
///
/// 1. Create a struct with configuration parameters
/// 2. Implement the `adjust()` method
/// 3. Return the adjusted score (0.0 to 1.0)
///
/// # Example
///
/// ```ignore
/// struct PopularityBoostAdjuster {
///     boost_factor: f32,
/// }
///
/// impl ScoreAdjuster for PopularityBoostAdjuster {
///     fn adjust(&self, base_score: f32, metadata: &VectorMetadata) -> f32 {
///         // Boost score based on content popularity
///         let popularity = metadata.metadata.get("popularity")
///             .and_then(|p| p.parse().ok())
///             .unwrap_or(0.0);
///         base_score * (1.0 + self.boost_factor * popularity)
///     }
/// }
/// ```
pub trait ScoreAdjuster: Send + Sync {
    /// Adjust the base score for a given search result
    ///
    /// This method takes a base similarity score (typically 0.0 to 1.0) and
    /// metadata about the search result, then returns an adjusted score.
    ///
    /// # Arguments
    /// * `base_score` - The initial similarity score (0.0 to 1.0)
    /// * `metadata` - Metadata about the search result (timestamp, content_type, etc.)
    ///
    /// # Returns
    /// The adjusted score (should typically be in range 0.0 to 1.0)
    ///
    /// # Example
    ///
    /// ```ignore
    /// fn adjust(&self, base_score: f32, metadata: &VectorMetadata) -> f32 {
    ///     if self.lambda <= 0.0 {
    ///         return base_score;
    ///     }
    ///     let days_since = (self.now - metadata.timestamp).num_days().max(0) as f32;
    ///     let decay_factor = (-self.lambda * days_since / 365.0).exp();
    ///     base_score * decay_factor
    /// }
    /// ```
    fn adjust(&self, base_score: f32, metadata: &VectorMetadata) -> f32;
}

/// Temporal decay adjuster - prioritizes recent content using exponential decay
///
/// # Formula
/// `adjusted_score = base_score × e^(-λ × days/365)`
///
/// Where:
/// - λ (lambda) is the recency bias parameter
/// - days is the age of the content
///
/// # Effects
/// - λ = 0.0: No decay (disabled)
/// - λ = 0.5: Moderate decay
/// - λ = 1.0: Aggressive decay
pub struct TemporalDecayAdjuster {
    /// Recency bias parameter (λ in the formula)
    /// Higher values = more aggressive decay of older content
    lambda: f32,

    /// Reference time for decay calculation
    /// Content timestamps are compared against this
    now: DateTime<Utc>,
}

impl TemporalDecayAdjuster {
    /// Create a new temporal decay adjuster
    ///
    /// # Arguments
    /// * `lambda` - Recency bias parameter (0.0 to 10.0)
    /// * `now` - Reference time for decay calculations
    ///
    /// # Example
    /// ```
    /// use chrono::Utc;
    /// let adjuster = TemporalDecayAdjuster::new(0.5, Utc::now());
    /// ```
    pub fn new(lambda: f32, now: DateTime<Utc>) -> Self {
        Self { lambda, now }
    }

    /// Create adjuster with current time
    pub fn with_current_time(lambda: f32) -> Self {
        Self::new(lambda, Utc::now())
    }
}

impl ScoreAdjuster for TemporalDecayAdjuster {
    fn adjust(&self, base_score: f32, metadata: &VectorMetadata) -> f32 {
        // If lambda is 0 or negative, decay is disabled
        if self.lambda <= 0.0 {
            return base_score;
        }

        // Calculate days since content was created
        let days_since = (self.now - metadata.timestamp).num_days().max(0) as f32;

        // Apply exponential decay formula: e^(-λ × days/365)
        // This means:
        // - Content from today: decay_factor = 1.0 (no decay)
        // - Content from 1 year ago with λ=1.0: decay_factor = e^(-1) ≈ 0.37
        // - Content from 1 year ago with λ=0.5: decay_factor = e^(-0.5) ≈ 0.61
        let decay_factor = (-self.lambda * days_since / 365.0).exp();

        // Apply decay to base score
        base_score * decay_factor
    }
}

/// Composite score adjuster - combines multiple adjusters sequentially
///
/// This adjuster applies multiple scoring strategies in sequence, with each
/// adjuster receiving the output of the previous one. This allows you to
/// compose complex scoring behaviors from simple, reusable components.
///
/// # Example
///
/// ```ignore
/// use post_cortex::core::scoring::{CompositeScoreAdjuster, TemporalDecayAdjuster};
/// use chrono::Utc;
///
/// // Combine temporal decay with custom popularity boost
/// let composite = CompositeScoreAdjuster::new(vec![
///     Box::new(TemporalDecayAdjuster::new(0.5, Utc::now())),
///     Box::new(PopularityBoostAdjuster { boost_factor: 0.2 }),
/// ]);
///
/// let adjusted_score = composite.adjust(0.8, &metadata);
/// // First applies temporal decay, then popularity boost
/// ```
pub struct CompositeScoreAdjuster {
    /// The adjusters to apply, in order
    adjusters: Vec<Box<dyn ScoreAdjuster>>,
}

impl CompositeScoreAdjuster {
    /// Create a new composite adjuster from a list of adjusters
    ///
    /// Adjusters are applied in the order they appear in the vector.
    ///
    /// # Arguments
    /// * `adjusters` - Vector of boxed adjusters to apply sequentially
    ///
    /// # Example
    ///
    /// ```ignore
    /// let composite = CompositeScoreAdjuster::new(vec![
    ///     Box::new(TemporalDecayAdjuster::new(0.5, Utc::now())),
    ///     Box::new(QualityWeightAdjuster::new(0.3)),
    /// ]);
    /// ```
    pub fn new(adjusters: Vec<Box<dyn ScoreAdjuster>>) -> Self {
        Self { adjusters }
    }

    /// Create an empty composite adjuster (no adjustments)
    ///
    /// This is useful when you want to conditionally add adjusters.
    pub fn empty() -> Self {
        Self {
            adjusters: Vec::new(),
        }
    }

    /// Add an adjuster to the composite
    ///
    /// # Arguments
    /// * `adjuster` - The adjuster to add
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut composite = CompositeScoreAdjuster::empty();
    /// composite.add(Box::new(TemporalDecayAdjuster::new(0.5, Utc::now())));
    /// composite.add(Box::new(PopularityBoostAdjuster::new(0.2)));
    /// ```
    pub fn add(&mut self, adjuster: Box<dyn ScoreAdjuster>) {
        self.adjusters.push(adjuster);
    }
}

impl ScoreAdjuster for CompositeScoreAdjuster {
    fn adjust(&self, base_score: f32, metadata: &VectorMetadata) -> f32 {
        // Apply each adjuster in sequence, feeding the output of one
        // into the input of the next
        self.adjusters.iter().fold(base_score, |score, adjuster| {
            adjuster.adjust(score, metadata)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_temporal_decay_disabled() {
        let now = Utc::now();
        let adjuster = TemporalDecayAdjuster::new(0.0, now);

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: now - Duration::days(365), // 1 year old
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = adjuster.adjust(0.8, &metadata);
        assert_eq!(adjusted, 0.8); // No decay when lambda=0
    }

    #[test]
    fn test_temporal_decay_recent_content() {
        let now = Utc::now();
        let adjuster = TemporalDecayAdjuster::new(1.0, now);

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: now, // Fresh content
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = adjuster.adjust(0.8, &metadata);
        assert!((adjusted - 0.8).abs() < 0.01); // No decay for fresh content
    }

    #[test]
    fn test_temporal_decay_old_content() {
        let now = Utc::now();
        let adjuster = TemporalDecayAdjuster::new(1.0, now);

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: now - Duration::days(365), // 1 year old
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = adjuster.adjust(0.8, &metadata);
        // After 1 year with lambda=1.0: decay = e^(-1) ≈ 0.37
        // So adjusted = 0.8 * 0.37 ≈ 0.29
        assert!((adjusted - 0.296).abs() < 0.01); // e^(-1) = 0.3679..., 0.8 * 0.3679 = 0.294
    }

    #[test]
    fn test_temporal_decay_half_lambda() {
        let now = Utc::now();
        let adjuster = TemporalDecayAdjuster::new(0.5, now);

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: now - Duration::days(365), // 1 year old
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = adjuster.adjust(0.8, &metadata);
        // After 1 year with lambda=0.5: decay = e^(-0.5) ≈ 0.61
        // So adjusted = 0.8 * 0.61 ≈ 0.49
        assert!((adjusted - 0.488).abs() < 0.01); // e^(-0.5) = 0.6065..., 0.8 * 0.6065 = 0.485
    }

    #[test]
    fn test_temporal_decay_future_timestamp() {
        let now = Utc::now();
        let adjuster = TemporalDecayAdjuster::new(1.0, now);

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: now + Duration::days(30), // Future timestamp
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = adjuster.adjust(0.8, &metadata);
        // Future content should get decay_factor > 1.0 (boost)
        // days_since will be 0 (max(0, -30))
        assert!((adjusted - 0.8).abs() < 0.01); // No decay for future content
    }

    // A simple test adjuster that always multiplies by a fixed factor
    struct BoostAdjuster {
        factor: f32,
    }

    impl ScoreAdjuster for BoostAdjuster {
        fn adjust(&self, base_score: f32, _metadata: &VectorMetadata) -> f32 {
            base_score * self.factor
        }
    }

    #[test]
    fn test_composite_adjuster_empty() {
        let composite = CompositeScoreAdjuster::empty();

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: Utc::now(),
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = composite.adjust(0.8, &metadata);
        assert_eq!(adjusted, 0.8); // No adjusters, score unchanged
    }

    #[test]
    fn test_composite_adjuster_single() {
        let adjuster = BoostAdjuster { factor: 0.5 };
        let composite = CompositeScoreAdjuster::new(vec![Box::new(adjuster)]);

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: Utc::now(),
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = composite.adjust(0.8, &metadata);
        assert_eq!(adjusted, 0.4); // 0.8 * 0.5
    }

    #[test]
    fn test_composite_adjuster_multiple() {
        let now = Utc::now();
        let composite = CompositeScoreAdjuster::new(vec![
            // First adjuster: decay by ~50% (e^(-0.693) ≈ 0.5)
            Box::new(TemporalDecayAdjuster::new(0.693, now)),
            // Second adjuster: boost by 2.0x
            Box::new(BoostAdjuster { factor: 2.0 }),
        ]);

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: now - Duration::days(365), // 1 year old
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = composite.adjust(0.8, &metadata);
        // First: decay from 1 year with lambda=0.693: 0.8 * 0.5 = 0.4
        // Then: boost by 2.0x: 0.4 * 2.0 = 0.8
        assert!((adjusted - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_composite_adjuster_add() {
        let mut composite = CompositeScoreAdjuster::empty();
        composite.add(Box::new(BoostAdjuster { factor: 0.5 }));
        composite.add(Box::new(BoostAdjuster { factor: 0.5 }));

        let metadata = VectorMetadata {
            id: "test".to_string(),
            source: "session123".to_string(),
            content_type: "qa".to_string(),
            timestamp: Utc::now(),
            text: "test content".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let adjusted = composite.adjust(0.8, &metadata);
        assert_eq!(adjusted, 0.2); // 0.8 * 0.5 * 0.5
    }
}
