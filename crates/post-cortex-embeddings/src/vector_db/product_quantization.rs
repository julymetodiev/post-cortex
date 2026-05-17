// Copyright (c) 2025, 2026 Julius ML
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

//! Product Quantization codebook for memory-efficient vector storage.

use anyhow::Result;
use tracing::{debug, warn};

use super::common::euclidean_distance;

/// Product Quantization codebook for memory-efficient vector storage
///
/// Splits each vector into subvectors and quantizes each independently
/// using learned centroids. Reduces memory by 8-32x with minimal accuracy loss.
#[derive(Debug, Clone)]
pub struct ProductQuantizationCodebook {
    /// Number of subvectors (dimension must be divisible by this)
    subvectors: usize,
    /// Bits per code (2^bits = number of centroids per subvector)
    bits: usize,
    /// Vector dimension
    dimension: usize,
    /// Centroids[subvector_idx][centroid_idx] = centroid vector
    /// Shape: [subvectors][2^bits][dimension/subvectors]
    centroids: Vec<Vec<Vec<f32>>>,
}

impl ProductQuantizationCodebook {
    /// Create new PQ codebook with random initialization
    ///
    /// # Warning
    /// This initializes centroids randomly, which provides poor quantization accuracy.
    /// For production use, centroids should be trained using k-means on representative data.
    ///
    /// # TODO
    /// Implement `train_from_data(vectors: &[Vec<f32>])` method for proper PQ training.
    pub fn new(dimension: usize, subvectors: usize, bits: usize) -> Result<Self> {
        if !dimension.is_multiple_of(subvectors) {
            return Err(anyhow::anyhow!(
                "Dimension {} must be divisible by subvectors {}",
                dimension,
                subvectors
            ));
        }

        let subvec_dim = dimension / subvectors;
        let num_centroids = 1 << bits; // 2^bits (e.g., 2^8 = 256)

        // WARNING: Random centroids provide poor quantization - consider training
        warn!(
            "PQ codebook initialized with random centroids - search accuracy will be degraded. \
             For production use, train centroids using k-means on representative data."
        );

        debug!(
            "Initializing PQ codebook: {} subvectors, {} bits ({} centroids), {} dim per subvec",
            subvectors, bits, num_centroids, subvec_dim
        );

        // Initialize random centroids (in practice, these would be trained via k-means)
        let mut centroids = Vec::with_capacity(subvectors);
        for _ in 0..subvectors {
            let mut subvec_centroids = Vec::with_capacity(num_centroids);
            for _ in 0..num_centroids {
                // Initialize with normalized random vectors
                let centroid: Vec<f32> = (0..subvec_dim)
                    .map(|_| rand::random::<f32>() * 2.0 - 1.0)
                    .collect();

                // Normalize to unit length
                let magnitude = centroid.iter().map(|x| x * x).sum::<f32>().sqrt();
                let normalized = if magnitude > 0.0 {
                    centroid.iter().map(|x| x / magnitude).collect()
                } else {
                    centroid
                };

                subvec_centroids.push(normalized);
            }
            centroids.push(subvec_centroids);
        }

        Ok(Self {
            subvectors,
            bits,
            dimension,
            centroids,
        })
    }

    /// Encode a vector into PQ codes
    pub fn encode(&self, vector: &[f32]) -> Vec<u8> {
        let subvec_dim = self.dimension / self.subvectors;
        let mut codes = Vec::with_capacity(self.subvectors);

        for i in 0..self.subvectors {
            let start = i * subvec_dim;
            let end = start + subvec_dim;
            let subvec = &vector[start..end];

            // Find nearest centroid using Euclidean distance
            let code = self.find_nearest_centroid(i, subvec);
            codes.push(code);
        }

        codes
    }

    /// Decode PQ codes back to approximate vector
    pub fn decode(&self, codes: &[u8]) -> Vec<f32> {
        let subvec_dim = self.dimension / self.subvectors;
        let mut vector = Vec::with_capacity(self.dimension);

        for (i, &code) in codes.iter().enumerate() {
            if i >= self.subvectors {
                warn!(
                    "PQ decode: code index {} >= subvectors {}",
                    i, self.subvectors
                );
                break;
            }

            let code_idx = code as usize;
            if code_idx >= self.centroids[i].len() {
                warn!(
                    "PQ decode: code {} >= centroids {} for subvector {}",
                    code_idx,
                    self.centroids[i].len(),
                    i
                );
                // Pad with zeros if invalid code
                vector.extend(vec![0.0; subvec_dim]);
                continue;
            }

            let centroid = &self.centroids[i][code_idx];
            vector.extend_from_slice(centroid);
        }

        vector
    }

    /// Find nearest centroid for a subvector
    fn find_nearest_centroid(&self, subvec_idx: usize, subvec: &[f32]) -> u8 {
        let centroids = &self.centroids[subvec_idx];
        let mut best_code = 0u8;
        let mut best_dist = f32::INFINITY;

        for (code, centroid) in centroids.iter().enumerate() {
            let dist = euclidean_distance(subvec, centroid);
            if dist < best_dist {
                best_dist = dist;
                best_code = code as u8;
            }
        }

        best_code
    }

    /// Approximate distance between query vector and PQ-encoded vector
    pub fn approximate_distance(&self, query: &[f32], codes: &[u8]) -> f32 {
        let subvec_dim = self.dimension / self.subvectors;
        let mut total_dist = 0.0;

        for (i, &code) in codes.iter().enumerate().take(self.subvectors) {
            let start = i * subvec_dim;
            let end = start + subvec_dim;
            let query_subvec = &query[start..end];

            let code_idx = code as usize;
            if code_idx < self.centroids[i].len() {
                let centroid = &self.centroids[i][code_idx];
                let dist = euclidean_distance(query_subvec, centroid);
                total_dist += dist * dist; // Sum of squared distances
            }
        }

        total_dist.sqrt()
    }

    /// Get compression ratio information
    pub fn compression_info(&self) -> (usize, usize, f32) {
        let original_size = self.dimension * std::mem::size_of::<f32>();
        let compressed_size = self.subvectors * self.bits.div_ceil(8); // Round up to bytes
        let ratio = original_size as f32 / compressed_size as f32;
        (original_size, compressed_size, ratio)
    }
}
