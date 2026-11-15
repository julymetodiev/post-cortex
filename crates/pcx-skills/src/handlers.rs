// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The skills' `pcx.skills.v1.Skills` service (W23) and their prompts:
//! each RPC's message or a prompt request in, the typed call of the skills
//! service, its message or a prompt out. No logic of their own.

use std::collections::HashMap;

use pcx_documents::Item;
use pcx_module_api::{
    ModuleError, ModuleErrorKind, Prompt, PromptContent, PromptMessage, PromptRole, Scope,
};
use post_cortex_proto::skills::skills_server::Skills;
use post_cortex_proto::skills::{
    AddSkillRequest, GetSkillRequest, ListSkillsRequest, SkillList, SkillRecord, UpdateSkillRequest,
};
use tonic::{Request, Response, Status};

use crate::domain::{Revision, Skill};
use crate::module::{self, Shared};
use crate::service::Service;

/// The skills' RPCs over the running module.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

fn record(item: Item<Skill>) -> SkillRecord {
    SkillRecord {
        id: item.document.id,
        version: item.document.version,
        skill: Some(item.data),
        instructions: item.document.text,
        updated_at: item.document.updated_at,
        author: item.document.author,
    }
}

#[tonic::async_trait]
impl Skills for Handlers {
    async fn add_skill(
        &self,
        request: Request<AddSkillRequest>,
    ) -> Result<Response<SkillRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let running = module::running(&self.shared)?;
        let docs = running.docs(&scope);
        let added = Service::new(&docs)
            .add(
                &request.name,
                &request.description,
                &request.instructions,
                request.tools,
            )
            .await?;
        Ok(Response::new(record(added)))
    }

    async fn update_skill(
        &self,
        request: Request<UpdateSkillRequest>,
    ) -> Result<Response<SkillRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let revision = Revision {
            description: request.description,
            instructions: request.instructions,
            tools: request.tools.map(|t| t.names),
        };
        let running = module::running(&self.shared)?;
        let docs = running.docs(&scope);
        let changed = Service::new(&docs)
            .update(&request.name, request.version, revision)
            .await?;
        Ok(Response::new(record(changed)))
    }

    async fn list_skills(
        &self,
        request: Request<ListSkillsRequest>,
    ) -> Result<Response<SkillList>, Status> {
        let scope = Scope::of(&request)?;
        let running = module::running(&self.shared)?;
        let docs = running.docs(&scope);
        let skills = Service::new(&docs).list().await?;
        Ok(Response::new(SkillList {
            skills: skills.into_iter().map(record).collect(),
        }))
    }

    async fn get_skill(
        &self,
        request: Request<GetSkillRequest>,
    ) -> Result<Response<SkillRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let running = module::running(&self.shared)?;
        let docs = running.docs(&scope);
        let skill = Service::new(&docs).get(&request.name).await?;
        Ok(Response::new(record(skill)))
    }
}

/// Every skill as a prompt: its name and description, no arguments.
pub async fn prompts(library: &Service<'_>) -> Result<Vec<Prompt>, ModuleError> {
    Ok(library
        .list()
        .await?
        .into_iter()
        .map(|s| Prompt {
            name: s.data.name,
            description: s.data.description,
            arguments: Vec::new(),
        })
        .collect())
}

/// Skill `name`'s prompt, one user message.
///
/// # Errors
/// `InvalidArgument` for any argument: a skill's prompt takes none.
pub async fn prompt(
    library: &Service<'_>,
    name: &str,
    arguments: &HashMap<String, String>,
) -> Result<PromptContent, ModuleError> {
    if let Some(given) = arguments.keys().next() {
        return Err(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("a skill's prompt takes no arguments, not {given}"),
        ));
    }
    let filled = library.prompt(name).await?;
    Ok(PromptContent {
        description: filled.description,
        messages: vec![PromptMessage {
            role: PromptRole::User.into(),
            text: filled.text,
        }],
    })
}
