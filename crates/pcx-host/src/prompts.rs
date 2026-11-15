// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Prompts (section 7): the running modules that offer prompts
//! (`Manifest.prompts`) list them in load order, each under its policy;
//! a `get` goes to the first module that lists the name.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pcx_module_api::{ModuleError, ModuleErrorKind, Prompt, PromptContent, Scope};

use crate::policy::Guarded;

/// The prompts of `modules`, in load order: a name a later module also
/// gives is left out, and so is a module whose listing fails; both are
/// logged.
pub async fn list(modules: &[Arc<Guarded>], scope: &Scope) -> Vec<Prompt> {
    let mut seen = HashSet::new();
    let mut prompts = Vec::new();
    for (guarded, listed) in listings(modules, scope).await {
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                tracing::warn!(module = guarded.id(), %error, "cannot list its prompts");
                continue;
            }
        };
        for prompt in listed {
            if seen.insert(prompt.name.clone()) {
                prompts.push(prompt);
            } else {
                tracing::warn!(
                    module = guarded.id(),
                    prompt = %prompt.name,
                    "an earlier module gives this prompt name"
                );
            }
        }
    }
    prompts
}

/// Prompt `name` of the first of `modules` that lists it, filled with
/// `arguments`.
///
/// # Errors
/// The module's error; when no module lists it, the first listing's
/// error, else `NotFound`.
pub async fn get(
    modules: &[Arc<Guarded>],
    scope: &Scope,
    name: &str,
    arguments: &HashMap<String, String>,
) -> Result<PromptContent, ModuleError> {
    // A module that cannot list may be the one that offers it.
    let mut unlisted = None;
    for (guarded, listed) in listings(modules, scope).await {
        match listed {
            Ok(listed) if listed.iter().any(|p| p.name == name) => {
                let (asked, name, arguments) = (scope.clone(), name.to_string(), arguments.clone());
                return guarded
                    .run(&scope.cancel, scope.deadline, "prompt", |m| {
                        Box::pin(async move { m.prompt(&asked, &name, &arguments).await })
                    })
                    .await;
            }
            Ok(_) => {}
            Err(error) => {
                unlisted.get_or_insert(error);
            }
        }
    }
    Err(unlisted.unwrap_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("no module offers the prompt {name}"),
        )
    }))
}

/// What each module that offers prompts lists, in load order.
async fn listings<'a>(
    modules: &'a [Arc<Guarded>],
    scope: &Scope,
) -> Vec<(&'a Guarded, Result<Vec<Prompt>, ModuleError>)> {
    let mut listings = Vec::new();
    for guarded in modules.iter().filter(|g| g.module().manifest().prompts) {
        let asked = scope.clone();
        let listed = guarded
            .run(&scope.cancel, scope.deadline, "prompts", |m| {
                Box::pin(async move { m.prompts(&asked).await })
            })
            .await;
        listings.push((guarded.as_ref(), listed));
    }
    listings
}
