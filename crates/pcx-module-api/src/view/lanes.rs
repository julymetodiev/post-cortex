// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The check of lanes (W16; W24: a card, groups, a person filter, a move
//! on a drop) and of images (W24).

use crate::view::{Card, Checker, Images, Lanes, Slot, ViewError};

impl Checker<'_> {
    /// Lanes: their rows and lanes, a title; groups need their source, a
    /// move a card with an id, each needs what it names.
    pub fn lanes(&self, lanes: &Lanes, at: &str) -> Result<(), ViewError> {
        self.source(lanes.source.as_ref(), at)?;
        self.named(&lanes.lane_field, at, "lane_field")?;
        self.source(lanes.lanes.as_ref(), at)?;
        self.named(&lanes.title_field, at, "title_field")?;
        self.actions(&lanes.row_actions, at, self.row_actions())?;
        match (lanes.group_field.trim().is_empty(), &lanes.groups) {
            (true, None) => {}
            (false, Some(groups)) => self.source(Some(groups), at)?,
            (true, Some(_)) => return Err(self.refused(at, "has groups but no group_field")),
            (false, None) => return Err(self.refused(at, "has a group_field but no groups")),
        }
        if let Some(card) = &lanes.card {
            self.card(card, at)?;
        }
        if let Some(action) = &lanes.r#move {
            if lanes
                .card
                .as_ref()
                .is_none_or(|c| c.id_field.trim().is_empty())
            {
                return Err(self.refused(at, "moves rows but has no card with an id_field"));
            }
            self.action(
                action,
                at,
                Slot {
                    row: true,
                    drop: true,
                    ..Slot::NONE
                },
            )?;
        }
        Ok(())
    }

    /// A card: what waits needs the id it names.
    fn card(&self, card: &Card, at: &str) -> Result<(), ViewError> {
        let waits = !card.waits_field.trim().is_empty() || !card.waited_by_field.trim().is_empty();
        if waits && card.id_field.trim().is_empty() {
            return Err(self.refused(at, "a card that shows what waits needs an id_field"));
        }
        Ok(())
    }

    /// Images: their source, and what adding them does.
    pub fn images(&self, images: &Images, at: &str) -> Result<(), ViewError> {
        self.source(images.source.as_ref(), at)?;
        if let Some(attach) = &images.attach {
            self.action(
                attach,
                at,
                Slot {
                    row: self.detail,
                    images: true,
                    ..Slot::NONE
                },
            )?;
        }
        Ok(())
    }
}
