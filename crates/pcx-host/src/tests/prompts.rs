// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Prompts: asked only of the modules that offer them, listed in load
//! order with a name kept by its first module, a `get` routed to the
//! module that lists it; a module that cannot list is left out.

use super::*;

use pcx_module_api::{Prompt, PromptContent, PromptMessage, PromptRole};

/// A module with prompts `names` (none: it cannot list them), offering
/// them or not in its manifest.
struct Prompter {
    manifest: Manifest,
    names: Option<Vec<&'static str>>,
}

fn prompter(id: &str, offers: bool, names: Option<Vec<&'static str>>) -> Arc<dyn Module> {
    let mut manifest = fake(id, ModuleKind::Tool, Act::Answer, FailurePolicy::FailOpen).manifest;
    manifest.events = Vec::new();
    manifest.prompts = offers;
    Arc::new(Prompter { manifest, names })
}

#[async_trait]
impl Module for Prompter {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri))
    }

    async fn prompts(&self, _: &Scope) -> Result<Vec<Prompt>, ModuleError> {
        let names = self
            .names
            .clone()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "cannot list"))?;
        Ok(names
            .into_iter()
            .map(|name| Prompt {
                name: name.to_string(),
                description: self.manifest.id.clone(),
                arguments: Vec::new(),
            })
            .collect())
    }

    async fn prompt(
        &self,
        _: &Scope,
        name: &str,
        _: &HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError> {
        Ok(PromptContent {
            description: String::new(),
            messages: vec![PromptMessage {
                role: PromptRole::User.into(),
                text: format!("{} from {}", name, self.manifest.id),
            }],
        })
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

#[tokio::test]
async fn prompts_are_listed_in_load_order_and_routed_to_their_module() {
    let host = Host::start(
        vec![
            prompter("first", true, Some(vec!["review", "plan"])),
            // Offers none in its manifest: never asked.
            prompter("quiet", false, Some(vec!["hidden"])),
            prompter("second", true, Some(vec!["plan", "ship"])),
        ],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let scope = scope(Duration::from_secs(1));

    let listed: Vec<(String, String)> = host
        .prompts(&scope)
        .await
        .into_iter()
        .map(|p| (p.name, p.description))
        .collect();
    let expected = [("review", "first"), ("plan", "first"), ("ship", "second")];
    assert_eq!(
        listed,
        expected.map(|(n, m)| (n.to_string(), m.to_string()))
    );

    let none = HashMap::new();
    let text = |c: PromptContent| c.messages[0].text.clone();
    // The first module that gives a name serves it.
    assert_eq!(
        text(host.prompt(&scope, "plan", &none).await.unwrap()),
        "plan from first"
    );
    assert_eq!(
        text(host.prompt(&scope, "ship", &none).await.unwrap()),
        "ship from second"
    );
    let hidden = host.prompt(&scope, "hidden", &none).await.unwrap_err();
    assert_eq!(hidden.kind, ModuleErrorKind::NotFound);
}

#[tokio::test]
async fn a_module_that_cannot_list_is_left_out_and_named_when_asked() {
    let host = Host::start(
        vec![
            prompter("broken", true, None),
            prompter("fine", true, Some(vec!["review"])),
        ],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let scope = scope(Duration::from_secs(1));

    let listed: Vec<String> = host
        .prompts(&scope)
        .await
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(listed, ["review"]);
    let none = HashMap::new();
    host.prompt(&scope, "review", &none).await.unwrap();
    // Not listed anywhere: the failed listing answers, not NotFound.
    let unknown = host.prompt(&scope, "other", &none).await.unwrap_err();
    assert_eq!(unknown.kind, ModuleErrorKind::Unavailable, "{unknown:?}");
}
