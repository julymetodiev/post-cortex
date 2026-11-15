//! A checked expression's references, by the id of their node, for the
//! evaluator (pcx patch).
//!
//! The evaluator looks a node up for every node it visits. A `HashMap<i64, _>`
//! hashes the id with SipHash each time; the references, sorted by id once
//! when the program is made, are found by a binary search over the few an
//! expression has.

use std::collections::HashMap;

use crate::checker::ReferenceInfo;

/// The references of a checked expression, sorted by node id.
#[derive(Debug, Clone, Default)]
pub(crate) struct References {
    entries: Vec<(i64, ReferenceInfo)>,
}

impl References {
    /// The references of `map`, sorted.
    pub(crate) fn new(map: &HashMap<i64, ReferenceInfo>) -> Self {
        let mut entries: Vec<(i64, ReferenceInfo)> =
            map.iter().map(|(id, r)| (*id, r.clone())).collect();
        entries.sort_by_key(|(id, _)| *id);
        Self { entries }
    }

    /// The reference of node `id`.
    pub(crate) fn get(&self, id: &i64) -> Option<&ReferenceInfo> {
        self.entries
            .binary_search_by_key(id, |(at, _)| *at)
            .ok()
            .map(|i| &self.entries[i].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_each_reference_by_its_id() {
        let mut map = HashMap::new();
        for id in [7, 2, 40, 13] {
            map.insert(id, ReferenceInfo::ident(format!("n{id}")));
        }
        let refs = References::new(&map);
        for id in [7, 2, 40, 13] {
            assert_eq!(refs.get(&id).map(|r| r.name.as_str()), Some(format!("n{id}").as_str()));
        }
        assert!(refs.get(&1).is_none());
        assert!(refs.get(&41).is_none());
    }
}
