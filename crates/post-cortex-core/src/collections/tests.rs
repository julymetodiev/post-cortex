// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// A change to one version leaves the other as it was, both ways.
#[test]
fn versions_share_and_stay_apart() {
    let mut a: SharedMap<String, u32> = (0..100).map(|i| (format!("k{i}"), i)).collect();
    let b = a.clone();
    a.insert("k1".into(), 1000);
    a.upsert("new".into(), || 0, |v| *v += 7);
    a.upsert("k2".into(), || 0, |v| *v += 1);
    assert_eq!(a.remove("k3"), Some(3));
    assert_eq!((a["k1"], a["new"], a["k2"]), (1000, 7, 3));
    assert!(!a.contains_key("k3"));
    assert_eq!((b["k1"], b["k2"], b["k3"]), (1, 2, 3));
    assert!(!b.contains_key("new"));
    assert_eq!((a.len(), b.len()), (100, 100));

    let mut v: SharedVec<u32> = (0..3000).collect();
    let w = v.clone();
    v.push(3000);
    if let Some(x) = v.get_mut(5) {
        *x = 99;
    }
    assert_eq!((v.len(), w.len()), (3001, 3000));
    assert_eq!((v[5], w[5]), (99, 5));
    assert_eq!(v.last(), Some(&3000));
    assert_eq!(w.last(), Some(&2999));
}

/// Iteration is in order both ways, across block edges, and knows its
/// length; `retain` keeps the order.
#[test]
fn shared_vec_iterates_in_order() {
    let v: SharedVec<usize> = (0..2500).collect();
    assert!(v.iter().copied().eq(0..2500));
    assert!(v.iter().rev().copied().eq((0..2500).rev()));
    let mut both = v.iter();
    assert_eq!((both.next(), both.next_back()), (Some(&0), Some(&2499)));
    assert_eq!(both.len(), 2498);
    let mut odd = v.clone();
    odd.retain(|x| x % 2 == 1);
    assert!(odd.iter().copied().eq((0..2500).filter(|x| x % 2 == 1)));
    assert!(SharedVec::<u8>::new().iter().next().is_none());
}

/// The serialised form is a `BTreeMap`'s (key order) and a `Vec`'s.
#[test]
fn serialised_like_the_std_collections() {
    let pairs: Vec<(String, u32)> = (0..50).rev().map(|i| (format!("k{i:02}"), i)).collect();
    let shared: SharedMap<String, u32> = pairs.iter().cloned().collect();
    let std: std::collections::BTreeMap<String, u32> = pairs.into_iter().collect();
    assert_eq!(
        serde_json::to_string(&shared).unwrap(),
        serde_json::to_string(&std).unwrap()
    );
    let back: SharedMap<String, u32> =
        serde_json::from_str(&serde_json::to_string(&std).unwrap()).unwrap();
    assert_eq!(back.len(), 50);
    let v: SharedVec<u32> = (0..10).collect();
    assert_eq!(serde_json::to_string(&v).unwrap(), "[0,1,2,3,4,5,6,7,8,9]");
}

/// Across group edges (64 x 64 elements): iteration both ways, from any
/// start, meeting in the middle, and in-place changes.
#[test]
fn shared_vec_spans_groups() {
    let n = 3 * 4096 + 77;
    let v: SharedVec<usize> = (0..n).collect();
    assert_eq!(v.len(), n);
    assert!(v.iter().copied().eq(0..n));
    assert!(v.iter().rev().copied().eq((0..n).rev()));
    for start in [0, 1, 63, 64, 65, 4095, 4096, 4097, 8191, n - 1, n, n + 5] {
        assert!(v.iter_from(start).copied().eq(start.min(n)..n), "{start}");
        assert_eq!(v.iter_from(start).len(), n - start.min(n));
    }
    let mut it = v.iter();
    let mut seen = Vec::new();
    while let (Some(a), Some(b)) = (it.next(), it.next_back()) {
        seen.push(*a);
        seen.push(*b);
    }
    seen.sort_unstable();
    assert_eq!(seen.len() + usize::from(n % 2 == 1), n);
    let mut w = v.clone();
    if let Some(x) = w.get_mut(5000) {
        *x = 0;
    }
    assert_eq!((v[5000], w[5000]), (5000, 0));
    assert_eq!((v.get(n), v.last()), (None, Some(&(n - 1))));
}
