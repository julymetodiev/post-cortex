// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Where entries are: the sessions, interned to small ids the arena's
//! session column holds, and each session's content ids with their slots.
//! An entry is keyed by (session, content id); the same content id in
//! another session is another entry.

use std::collections::HashMap;
use std::sync::Arc;

/// The sessions and their entries.
#[derive(Default)]
pub struct Catalog {
    /// Session name -> interned id.
    ids: HashMap<Arc<str>, u32>,
    /// Interned id -> the session's content ids and their slots.
    contents: Vec<HashMap<Arc<str>, u32>>,
    /// Interned id -> session name.
    names: Vec<Arc<str>>,
}

impl Catalog {
    /// The interned id of `session`, interning it the first time.
    pub fn intern(&mut self, session: &str) -> u32 {
        if let Some(&id) = self.ids.get(session) {
            return id;
        }
        let id = self.names.len() as u32;
        let name: Arc<str> = Arc::from(session);
        self.ids.insert(Arc::clone(&name), id);
        self.names.push(name);
        self.contents.push(HashMap::new());
        id
    }

    /// The interned id of `session`, if it was ever seen.
    pub fn find(&self, session: &str) -> Option<u32> {
        self.ids.get(session).copied()
    }

    /// Put `slot` under `content_id` of session `id` (an interned id),
    /// in place of the slot it had.
    pub fn put(&mut self, id: u32, content_id: &str, slot: u32) {
        let Some(contents) = self.contents.get_mut(id as usize) else {
            return;
        };
        match contents.get_mut(content_id) {
            Some(held) => *held = slot,
            None => {
                contents.insert(Arc::from(content_id), slot);
            }
        }
    }

    /// Take `content_id` of session `id` out; returns its slot.
    pub fn release(&mut self, id: u32, content_id: &str) -> Option<u32> {
        self.contents.get_mut(id as usize)?.remove(content_id)
    }

    /// The slot of `content_id` in session `id`.
    pub fn slot(&self, id: u32, content_id: &str) -> Option<u32> {
        self.contents.get(id as usize)?.get(content_id).copied()
    }

    /// The slots of session `id`, in no particular order.
    pub fn slots(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        self.contents
            .get(id as usize)
            .into_iter()
            .flat_map(|contents| contents.values().copied())
    }

    /// Entries per session, for the sessions that have any.
    pub fn counts(&self) -> HashMap<String, usize> {
        self.names
            .iter()
            .zip(&self.contents)
            .filter(|(_, contents)| !contents.is_empty())
            .map(|(name, contents)| (name.to_string(), contents.len()))
            .collect()
    }
}
