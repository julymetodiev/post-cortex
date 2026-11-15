// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The presets (docs/modules/http.md, presets): the built-in ones, which
//! ship with the module and cannot change, and the organisation's, kept as
//! documents, each name taken once.

use pcx_documents::{Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::http::{
    Deleted, ListPresetsRequest, Preset, PresetList, PresetRecord, SavePresetRequest,
};

use crate::domain::{self, preset};
use crate::service::{Service, broken};

fn record(item: Item<Preset>) -> PresetRecord {
    PresetRecord {
        id: item.document.id,
        version: item.document.version,
        preset: Some(item.data),
        built_in: false,
    }
}

fn built(preset: &Preset) -> PresetRecord {
    PresetRecord {
        id: String::new(),
        version: 0,
        preset: Some(preset.clone()),
        built_in: true,
    }
}

impl Service<'_> {
    fn presets(&self) -> Typed<'_> {
        Typed::new(self.docs, domain::preset_kind())
    }

    /// The organisation's preset `name`, when there is one.
    async fn own(&self, name: &str) -> Result<Option<Item<Preset>>, ModuleError> {
        let example = Preset {
            name: name.to_string(),
            ..Preset::default()
        };
        Ok(self
            .presets()
            .all::<Preset>(Some((&example, &["name"][..])))
            .await?
            .into_iter()
            .next())
    }

    /// The presets, of `request`'s service when it names one, by service
    /// and name.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list_presets(
        &self,
        request: &ListPresetsRequest,
    ) -> Result<PresetList, ModuleError> {
        let of = |p: &Preset| request.service.is_empty() || p.service == request.service;
        let mut presets: Vec<PresetRecord> =
            self.built_in.iter().filter(|p| of(p)).map(built).collect();
        presets.extend(
            self.presets()
                .all::<Preset>(None)
                .await?
                .into_iter()
                .filter(|item| of(&item.data))
                .map(record),
        );
        presets.sort_by(|a, b| {
            let key = |r: &PresetRecord| {
                r.preset
                    .as_ref()
                    .map(|p| (p.service.clone(), p.name.clone()))
            };
            key(a).cmp(&key(b))
        });
        Ok(PresetList { presets })
    }

    /// Preset `name`, built in or the organisation's.
    ///
    /// # Errors
    /// `NotFound` when there is none; memory failed.
    pub async fn get_preset(&self, name: &str) -> Result<PresetRecord, ModuleError> {
        if let Some(preset) = self.built_in.iter().find(|p| p.name == name) {
            return Ok(built(preset));
        }
        self.own(name)
            .await?
            .map(record)
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::NotFound, format!("no preset {name}")))
    }

    /// Add `request`'s preset (version 0) or change it from the version
    /// read, once it checks.
    ///
    /// # Errors
    /// `InvalidArgument` for a preset the rules refuse or a built-in's
    /// name; `Conflict` for a new one whose name is taken, or one that
    /// changed since the version read; `NotFound` for a change of none;
    /// memory failed.
    pub async fn save_preset(
        &self,
        request: SavePresetRequest,
    ) -> Result<PresetRecord, ModuleError> {
        let given = request.preset.ok_or_else(|| {
            ModuleError::new(ModuleErrorKind::InvalidArgument, "a preset is required")
                .with_field("preset".to_string())
        })?;
        let built_in: Vec<String> = self.built_in.iter().map(|p| p.name.clone()).collect();
        preset::check(&given, &built_in).map_err(broken)?;
        let existing = self.own(&given.name).await?;
        let saved = match (existing, request.version) {
            (None, 0) => self.presets().create(&given, &given.description).await?,
            (Some(_), 0) => {
                return Err(ModuleError::new(
                    ModuleErrorKind::Conflict,
                    format!(
                        "a preset {} is kept already; read it and change it from its version",
                        given.name
                    ),
                ));
            }
            (Some(item), version) => {
                self.presets()
                    .change(&item.document.id, version, &given, &given.description)
                    .await?
            }
            (None, _) => {
                return Err(ModuleError::new(
                    ModuleErrorKind::NotFound,
                    format!("no preset {} to change", given.name),
                ));
            }
        };
        Ok(record(saved))
    }

    /// Remove the organisation's preset `name`, from `version` read.
    ///
    /// # Errors
    /// `InvalidArgument` for a built-in one; `NotFound` for none;
    /// `Conflict` when it changed since `version`; memory failed.
    pub async fn delete_preset(&self, name: &str, version: u64) -> Result<Deleted, ModuleError> {
        if self.built_in.iter().any(|p| p.name == name) {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!(
                    "{name} is a built-in preset: it is not removed (duplicate it to change it)"
                ),
            ));
        }
        let item = self.own(name).await?.ok_or_else(|| {
            ModuleError::new(ModuleErrorKind::NotFound, format!("no preset {name}"))
        })?;
        if item.document.version != version {
            return Err(ModuleError::new(
                ModuleErrorKind::Conflict,
                format!(
                    "preset {name} changed since version {version}; read version {} and decide again",
                    item.document.version
                ),
            ));
        }
        self.presets().delete(&item.document.id).await?;
        Ok(Deleted {
            name: name.to_string(),
        })
    }
}
