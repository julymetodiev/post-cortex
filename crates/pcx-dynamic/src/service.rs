// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `dynamic` module's capability (docs/modules/dynamic.md): a version
//! kept, prepared, checked in a locked run and asked about; what was approved
//! heard or read again; what a host runs.

use pcx_approval::Place;
use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::dynamic::{
    ApprovedList, ApprovedModule, DynamicList, DynamicModule, DynamicRecord, EditRequest, File,
    InstallRequest, ModuleFiles,
};
use tracing::{info, warn};

use crate::domain::{self, DEFAULT_MEMORY_MB, MODULE_KIND};
use crate::ports::{Blobs, Board, Bundles, Checking, Directory, Events, Prepared, Reach};

/// Times a module is read again when another write came between.
const TRIES: u32 = 4;

/// The module's settings, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The workspace its modules are kept in.
    pub workspace: String,
    /// The board a person approves them on.
    pub board: String,
    /// The most modules it keeps (the organisation's plan); none: no limit.
    pub max_modules: Option<usize>,
}

/// What the service reaches.
pub struct Ports<'a> {
    /// Its documents.
    pub documents: &'a dyn Documents,
    /// Memory's blobs.
    pub blobs: &'a dyn Blobs,
    /// The launcher.
    pub bundles: &'a dyn Bundles,
    /// The board.
    pub board: &'a dyn Board,
    /// What a caller reaches.
    pub reach: &'a dyn Reach,
    /// The files a caller's tools reach.
    pub directory: &'a dyn Directory,
    /// The bus.
    pub events: &'a dyn Events,
}

/// The capability.
pub struct Service<'a> {
    ports: Ports<'a>,
    settings: &'a Settings,
}

fn refused(message: impl Into<String>) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, message)
}

impl<'a> Service<'a> {
    /// The capability over `ports`, with `settings`.
    #[must_use]
    pub fn new(ports: Ports<'a>, settings: &'a Settings) -> Self {
        Self { ports, settings }
    }

    fn typed(&self) -> Typed<'_> {
        Typed::new(self.ports.documents, MODULE_KIND)
    }

    /// The module of `id`'s document.
    async fn find(&self, id: &str) -> Result<Option<Item<DynamicModule>>, ModuleError> {
        let example = DynamicModule {
            id: id.to_string(),
            ..Default::default()
        };
        Ok(self
            .typed()
            .all(Some((&example, &["id"])))
            .await?
            .into_iter()
            .next())
    }

    /// Whether a module `id` fits: a new version of one always does.
    async fn within_limit(&self, id: &str) -> Result<(), ModuleError> {
        let Some(most) = self.settings.max_modules else {
            return Ok(());
        };
        if self.find(id).await?.is_some()
            || self.typed().all::<DynamicModule>(None).await?.len() < most
        {
            return Ok(());
        }
        Err(ModuleError::new(
            ModuleErrorKind::ResourceExhausted,
            format!(
                "this organisation's plan holds {most} dynamic modules: remove one or change the plan"
            ),
        ))
    }

    /// Install `request` for `scope`'s author: refused as a whole, or kept
    /// and asked about.
    ///
    /// # Errors
    /// `ResourceExhausted` for a new module past the plan's limit;
    /// `InvalidArgument` for a bundle or grants that break a rule;
    /// `FailedPrecondition` when its dependencies cannot be met, it does not
    /// start, its manifest is refused or asks past its grants, or its tests
    /// fail; memory, the launcher or the board failed.
    pub async fn install(
        &self,
        scope: &Scope,
        mut request: InstallRequest,
    ) -> Result<DynamicRecord, ModuleError> {
        if !request.dir.is_empty() {
            if !request.files.is_empty() {
                return Err(refused("a bundle is its files or its dir, not both"));
            }
            request.files = self.ports.directory.files(scope, &request.dir).await?;
        }
        self.write(scope, request, None).await
    }

    /// A person's change of one file of module `request.id`'s latest files:
    /// a new version, as an install of it is.
    ///
    /// # Errors
    /// `NotFound`; `Conflict` when the module changed since `request.revision`;
    /// as `install`.
    pub async fn edit(
        &self,
        scope: &Scope,
        request: EditRequest,
    ) -> Result<DynamicRecord, ModuleError> {
        let item = self.existing(&request.id).await?;
        let files = self.files(item.data.files.as_ref()).await?;
        let language = item.data.language();
        self.write(
            scope,
            InstallRequest {
                id: request.id,
                version: request.version,
                language: language.into(),
                files: domain::with_file(files, &request.path, &request.text),
                grants: item.data.grants,
                dir: String::new(),
            },
            Some(request.revision),
        )
        .await
    }

    /// `request` kept and asked about; `read_at`: only over that version of
    /// its document.
    async fn write(
        &self,
        scope: &Scope,
        request: InstallRequest,
        read_at: Option<u64>,
    ) -> Result<DynamicRecord, ModuleError> {
        domain::check_id(&request.id).map_err(refused)?;
        self.within_limit(&request.id).await?;
        let language = request.language();
        domain::check_files(language, &request.files).map_err(refused)?;
        let grants = request.grants.clone().unwrap_or_default();
        domain::check_grants(&grants).map_err(refused)?;
        let reachable = self.ports.reach.modules(scope).await?;
        let beyond = domain::beyond_author(&grants, &reachable);
        if !beyond.is_empty() {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!("its author reaches no {}", beyond.join(", ")),
            ));
        }
        let tar = domain::tar_of(&request.files).map_err(refused)?;
        let sha256 = domain::sha256(&tar);
        let prepared = self.ports.bundles.prepare(tar.clone(), &sha256).await?;
        let memory_mb = memory_of(grants.memory_mb);
        let has_tests = request.files.iter().any(|f| f.path.starts_with("tests/"));
        let check = self
            .ports
            .bundles
            .check(
                Checking {
                    id: &request.id,
                    language,
                    grants: &grants,
                    has_tests,
                    memory_mb,
                },
                &prepared,
            )
            .await?;
        if !check.tests_passed {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("its tests failed: {}", check.log),
            ));
        }
        if let Some(extra) = check.uses.iter().find(|u| !grants.uses.contains(*u)) {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("its manifest uses {extra}, which its grants do not name"),
            ));
        }
        let files = self.ports.blobs.put(tar).await?;
        let written = DynamicModule {
            id: request.id,
            version: request.version,
            language: language.into(),
            files: Some(files),
            grants: Some(grants),
            check: Some(check),
            ..Default::default()
        };
        let previous = self.find(&written.id).await?;
        let waited = previous
            .as_ref()
            .and_then(|p| p.data.asked.as_ref())
            .map(|a| a.decision.clone());
        let kept = domain::kept(written, previous.as_ref().map(|p| &p.data));
        let text = format!("Dynamic module {} {}", kept.id, kept.version);
        let item = match previous {
            Some(p) => {
                let at = read_at.unwrap_or(p.document.version);
                self.typed()
                    .change(&p.document.id, at, &kept, &text)
                    .await?
            }
            None => self.typed().create(&kept, &text).await?,
        };
        let item = if domain::asks(&item.data) {
            self.ask(item).await?
        } else {
            item
        };
        if let Some(stale) = domain::superseded(waited.as_deref(), &item.data) {
            let why = format!(
                "Dynamic module {} changed since this was asked.",
                item.data.id
            );
            self.withdraw(&item.data.id, stale, why).await;
        }
        Ok(domain::record(item))
    }

    /// Decision `decision`, asked in module `id`'s name, withdrawn because of
    /// `why`: no one is asked any more. One that waits no more is left as it
    /// is; a board that does not answer is logged.
    async fn withdraw(&self, id: &str, decision: &str, why: String) {
        let request = pcx_approval::withdrawal(&self.settings.workspace, decision.to_string(), why);
        match self.ports.board.withdraw(&domain::asker(id), request).await {
            Ok(_) => {
                info!(module = %id, decision = %decision, "a decision no one waits on was withdrawn")
            }
            Err(e) if e.kind == ModuleErrorKind::FailedPrecondition => {}
            Err(e) => {
                warn!(module = %id, decision = %decision, "a decision no one waits on was not withdrawn: {e}");
            }
        }
    }

    /// The decision on `item` recorded on its task, and written in it;
    /// approved at once when a team's policy approves it.
    async fn ask(&self, item: Item<DynamicModule>) -> Result<Item<DynamicModule>, ModuleError> {
        let place = Place {
            workspace: &self.settings.workspace,
            board: &self.settings.board,
        };
        let asker = domain::asker(&item.data.id);
        let task = self
            .ports
            .board
            .task(
                &asker,
                pcx_approval::task(
                    place,
                    format!("Dynamic module {}", item.data.id),
                    format!("Version {}", item.data.version),
                    format!("dynamic:{}", item.document.id),
                ),
            )
            .await?;
        let decided = self
            .ports
            .board
            .decide(
                &asker,
                pcx_approval::question(
                    place,
                    task.id.clone(),
                    domain::question(&item.data),
                    format!(
                        "A dynamic module's code runs only as a person approved it. Read it in the module's view: {}.",
                        item.data.id
                    ),
                ),
            )
            .await?;
        let approved_by = pcx_approval::approved_at_once(&decided);
        let (document, id) = (item.document.id.clone(), item.data.id.clone());
        let changed = self
            .change(&document, |module| {
                let waiting = domain::asked(module.clone(), &task.id, &decided.id);
                match &approved_by {
                    Some(by) => domain::approve(waiting, &decided.id, by),
                    None => Some(waiting),
                }
            })
            .await?;
        if approved_by.is_some() {
            self.ports.events.changed(&id, false).await;
        }
        Ok(changed.unwrap_or(item))
    }

    /// Document `document` changed by `change` from its latest version (none:
    /// left as it is), read again when another write came between.
    async fn change(
        &self,
        document: &str,
        change: impl Fn(&DynamicModule) -> Option<DynamicModule>,
    ) -> Result<Option<Item<DynamicModule>>, ModuleError> {
        let mut tries = 0;
        loop {
            let Some(latest) = self.typed().find::<DynamicModule>(document).await? else {
                return Ok(None);
            };
            let Some(changed) = change(&latest.data) else {
                return Ok(None);
            };
            let text = format!("Dynamic module {} {}", changed.id, changed.version);
            match self
                .typed()
                .change(document, latest.document.version, &changed, &text)
                .await
            {
                Err(e) if e.kind == ModuleErrorKind::Conflict && tries < TRIES => tries += 1,
                written => return written.map(Some),
            }
        }
    }

    /// The modules, newest change first.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list(&self) -> Result<DynamicList, ModuleError> {
        let modules = self
            .typed()
            .all::<DynamicModule>(None)
            .await?
            .into_iter()
            .map(domain::record)
            .collect();
        Ok(DynamicList { modules })
    }

    /// Module `id`, with its latest files and their difference from the
    /// approved version.
    ///
    /// # Errors
    /// `NotFound`; memory failed.
    pub async fn get(&self, id: &str) -> Result<DynamicRecord, ModuleError> {
        let item = self.existing(id).await?;
        let files = self.files(item.data.files.as_ref()).await?;
        let approved_files = item.data.approved.as_ref().and_then(|a| a.files.as_ref());
        let before = self.files(approved_files).await?;
        Ok(DynamicRecord {
            difference: domain::difference(&before, &files),
            files,
            ..domain::record(item)
        })
    }

    /// Module `id`'s latest files, as its view shows them.
    ///
    /// # Errors
    /// `NotFound`; memory failed.
    pub async fn module_files(&self, id: &str) -> Result<ModuleFiles, ModuleError> {
        let item = self.existing(id).await?;
        let files = self.files(item.data.files.as_ref()).await?;
        let files = domain::rows(&item.data, item.document.version, files);
        Ok(ModuleFiles { files })
    }

    async fn existing(&self, id: &str) -> Result<Item<DynamicModule>, ModuleError> {
        self.find(id)
            .await?
            .ok_or_else(|| self.typed().not_found(id))
    }

    async fn files(
        &self,
        files: Option<&post_cortex_proto::pb::Attachment>,
    ) -> Result<Vec<File>, ModuleError> {
        let Some(files) = files else {
            return Ok(Vec::new());
        };
        let tar = self.ports.blobs.get(&files.sha256).await?;
        domain::files_of(&tar).map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))
    }

    /// Remove module `id`: a host stops it, memory drops its files, and the
    /// decision it waited on is withdrawn.
    ///
    /// # Errors
    /// `NotFound`; memory failed.
    pub async fn remove(&self, id: &str) -> Result<(), ModuleError> {
        let item = self.existing(id).await?;
        self.typed().delete(&item.document.id).await?;
        if let Some(asked) = &item.data.asked {
            let why = format!("Dynamic module {id} was removed.");
            self.withdraw(id, &asked.decision, why).await;
        }
        self.ports.events.changed(id, true).await;
        Ok(())
    }

    /// What a host runs: each approved version, prepared. A module that
    /// cannot be prepared is logged and left out, and the others run.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn approved(&self) -> Result<ApprovedList, ModuleError> {
        let mut modules = Vec::new();
        for item in self.typed().all::<DynamicModule>(None).await? {
            let Some(approved) = item.data.approved else {
                continue;
            };
            match self.prepared(&approved).await {
                Ok(prepared) => modules.push(ApprovedModule {
                    id: item.data.id,
                    language: approved.language,
                    grants: approved.grants,
                    bundle: prepared.bundle.display().to_string(),
                    environment: prepared
                        .environment
                        .map(|e| e.display().to_string())
                        .unwrap_or_default(),
                    sha256: approved.sha256,
                }),
                Err(e) => warn!(module = %item.data.id, "an approved module was not prepared: {e}"),
            }
        }
        Ok(ApprovedList { modules })
    }

    async fn prepared(
        &self,
        approved: &post_cortex_proto::dynamic::Approved,
    ) -> Result<Prepared, ModuleError> {
        let tar = self.ports.blobs.get(&approved.sha256).await?;
        self.ports.bundles.prepare(tar, &approved.sha256).await
    }
}

/// The memory limit of `asked` MB: 0, the default.
#[must_use]
pub fn memory_of(asked: u32) -> u32 {
    if asked == 0 { DEFAULT_MEMORY_MB } else { asked }
}

mod approvals;

#[cfg(test)]
mod tests;
