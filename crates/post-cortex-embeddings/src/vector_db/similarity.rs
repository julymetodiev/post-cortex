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

//! Cosine-similarity kernels shared by the vector DB and its callers.
//!
//! The loops keep `LANES` independent accumulators so LLVM can vectorise
//! them: a plain `iter().sum()` over `f32` is a single dependency chain the
//! compiler may not reorder. When scoring many vectors against one query,
//! compute the query norm once with [`l2_norm`] and call
//! [`cosine_with_norms`] with each stored vector's precomputed norm — that is
//! one dot product per candidate instead of three passes and two `sqrt`s.

const LANES: usize = 8;

/// Dot product of the common prefix of `a` and `b`.
#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut acc = [0.0f32; LANES];
    let (ca, ra) = a.as_chunks::<LANES>();
    let (cb, rb) = b.as_chunks::<LANES>();
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..LANES {
            acc[i] += x[i] * y[i];
        }
    }
    acc.iter().sum::<f32>() + tail
}

/// Euclidean (L2) norm of `v`.
#[inline]
pub fn l2_norm(v: &[f32]) -> f32 {
    dot(v, v).sqrt()
}

/// Cosine similarity given both norms up front: one pass over the data.
///
/// Returns `0.0` when the lengths differ or either norm is zero, matching
/// [`cosine_similarity`].
#[inline]
pub fn cosine_with_norms(a: &[f32], b: &[f32], norm_a: f32, norm_b: f32) -> f32 {
    if a.len() != b.len() || norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot(a, b) / (norm_a * norm_b)
}

/// Cosine similarity of two vectors in a single fused pass.
///
/// Returns `0.0` when the lengths differ or either vector is all zeros.
#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut ab = [0.0f32; LANES];
    let mut aa = [0.0f32; LANES];
    let mut bb = [0.0f32; LANES];
    let (ca, ra) = a.as_chunks::<LANES>();
    let (cb, rb) = b.as_chunks::<LANES>();
    let (mut tab, mut taa, mut tbb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in ra.iter().zip(rb) {
        tab += x * y;
        taa += x * x;
        tbb += y * y;
    }
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..LANES {
            ab[i] += x[i] * y[i];
            aa[i] += x[i] * x[i];
            bb[i] += y[i] * y[i];
        }
    }
    let norm_a = (aa.iter().sum::<f32>() + taa).sqrt();
    let norm_b = (bb.iter().sum::<f32>() + tbb).sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        (ab.iter().sum::<f32>() + tab) / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_cosine(a: &[f32], b: &[f32]) -> f32 {
        let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            d / (na * nb)
        }
    }

    fn sample(len: usize, seed: f32) -> Vec<f32> {
        (0..len).map(|i| ((i as f32 + seed) * 0.37).sin()).collect()
    }

    #[test]
    fn kernels_match_naive_for_lengths_around_the_lane_width() {
        for len in [0, 1, 3, 7, 8, 9, 15, 16, 17, 255, 256, 384] {
            let a = sample(len, 1.0);
            let b = sample(len, 2.5);
            let expected = naive_cosine(&a, &b);
            assert!((cosine_similarity(&a, &b) - expected).abs() < 1e-5, "{len}");
            let fused = cosine_with_norms(&a, &b, l2_norm(&a), l2_norm(&b));
            assert!((fused - expected).abs() < 1e-5, "{len}");
            let d: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            assert!((dot(&a, &b) - d).abs() < 1e-3, "{len}");
        }
    }

    #[test]
    fn degenerate_inputs_score_zero() {
        assert_eq!(cosine_similarity(&[0.0; 4], &[1.0; 4]), 0.0);
        assert_eq!(cosine_similarity(&[1.0; 4], &[1.0; 3]), 0.0);
        assert_eq!(cosine_with_norms(&[1.0; 4], &[1.0; 3], 2.0, 2.0), 0.0);
        assert_eq!(cosine_with_norms(&[1.0; 4], &[1.0; 4], 0.0, 2.0), 0.0);
    }
}
