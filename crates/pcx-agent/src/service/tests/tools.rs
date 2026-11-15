//! The tools a turn is given: a member's, and a call to one it was not.

use super::*;

#[tokio::test]
async fn a_member_turn_adds_its_instructions_model_and_allowed_tools() {
    let model = Scripted::with(vec![Ok(said("Done."))]);
    let (tools, hooks, kept) = (Echo::default(), Heard::default(), Kept::default());
    let (scope, settings) = (scope(), settings());
    let (tx, _rx) = mpsc::channel(64);
    let agent = Agent {
        models: &model,
        tools: &tools,
        hooks: &hooks,
        transcript: &kept,
        context: None,
        settings: &settings,
        scope: &scope,
    };
    let member = TurnRequest {
        instructions: "You review code.".into(),
        model: Some("big".into()),
        modules: vec!["tools-git".into()],
        ..TurnRequest::new("s1", "Look")
    };
    agent.run(&member, &tx).await.unwrap();
    let asked = model.asked.lock()[0].clone();
    assert_eq!(asked.model, "big");
    assert!(asked.system.ends_with("You review code."));
    assert!(asked.tools.is_empty(), "fs_read is not of tools-git");
}

/// A tool the turn was not given is refused even when the model calls it:
/// one withheld, or one of a module the turn may not use.
#[tokio::test]
async fn a_tool_the_turn_was_not_given_is_refused_when_called() {
    for member in [
        TurnRequest {
            withheld: vec!["fs_read".into()],
            ..TurnRequest::new("s1", "Read it")
        },
        TurnRequest {
            modules: vec!["tools-git".into()],
            ..TurnRequest::new("s1", "Read it")
        },
    ] {
        let model = Scripted::with(vec![Ok(read_call()), Ok(said("Could not."))]);
        let (tools, hooks, kept) = (Echo::default(), Heard::default(), Kept::default());
        let (scope, settings) = (scope(), settings());
        let (tx, _rx) = mpsc::channel(64);
        let agent = Agent {
            models: &model,
            tools: &tools,
            hooks: &hooks,
            transcript: &kept,
            context: None,
            settings: &settings,
            scope: &scope,
        };
        agent.run(&member, &tx).await.unwrap();
        assert!(
            model.asked.lock()[0].tools.is_empty(),
            "fs_read is not offered"
        );
        assert!(tools.called.lock().is_empty(), "nor run");
        let refused = kept
            .turns
            .lock()
            .iter()
            .flat_map(|t| t.blocks.clone())
            .find_map(|b| match b {
                Block::ToolResult {
                    content, is_error, ..
                } => Some((content, is_error)),
                _ => None,
            });
        assert_eq!(
            refused,
            Some(("fs_read is not a tool of this turn".to_string(), true))
        );
    }
}

/// A tool of a runner that is not connected.
struct Away;

#[async_trait::async_trait]
impl crate::ports::Tools for Away {
    async fn list(&self) -> Result<Vec<(String, ModelTool)>, ModuleError> {
        Echo::default().list().await
    }

    async fn call(&self, _: &str, _: serde_json::Value) -> crate::ports::ToolAnswer {
        crate::ports::ToolAnswer {
            content: "the runner is offline".to_string(),
            is_error: true,
            offline: true,
        }
    }
}

#[tokio::test]
async fn a_tool_on_a_runner_that_is_away_ends_the_turn_to_wait_with_its_call_answered() {
    let model = Scripted::with(vec![Ok(read_call()), Ok(said("never asked"))]);
    let (hooks, kept) = (Heard::default(), Kept::default());
    let (scope, settings) = (scope(), settings());
    let (tx, _rx) = mpsc::channel(64);
    let agent = Agent {
        models: &model,
        tools: &Away,
        hooks: &hooks,
        transcript: &kept,
        context: None,
        settings: &settings,
        scope: &scope,
    };
    let waits = agent
        .run(&TurnRequest::new("s1", "Read it"), &tx)
        .await
        .unwrap_err();
    assert_eq!(waits.kind, pcx_module_api::ModuleErrorKind::Offline);
    assert_eq!(
        model.asked.lock().len(),
        1,
        "the model is not asked to try again"
    );
    assert!(
        kept.turns
            .lock()
            .iter()
            .flat_map(|t| t.blocks.clone())
            .any(|b| matches!(b, Block::ToolResult { is_error: true, .. })),
        "its call stays answered, for the turn that goes on"
    );
}
