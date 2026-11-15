//! The model's window in a turn: a conversation that leaves too little of
//! it is compacted before the model is asked.

use super::*;

/// A transcript of `n` earlier exchanges of about `size` characters each.
fn history(n: usize, size: usize) -> Vec<Turn> {
    (0..n)
        .flat_map(|i| {
            [
                Turn::new(format!("p{i}"), Role::User, None, said(&"x".repeat(size)).0),
                Turn::new(format!("a{i}"), Role::Assistant, None, said("ok").0),
            ]
        })
        .collect()
}

#[tokio::test]
async fn a_conversation_near_the_window_is_compacted_before_the_model_is_asked() {
    // Three exchanges of about 10k tokens each, a 40k window: 30k plus the
    // 16k reserve does not fit.
    let model = Scripted {
        window: 40_000,
        ..Scripted::with(vec![Ok(said("Done."))])
    };
    let (tools, hooks, engine) = (Echo::default(), Heard::default(), Engine::default());
    let kept = Kept {
        turns: Mutex::new(history(3, 40_000)),
    };
    let (scope, settings) = (scope(), settings());
    let (tx, _rx) = mpsc::channel(64);
    let agent = Agent {
        models: &model,
        tools: &tools,
        hooks: &hooks,
        transcript: &kept,
        context: Some(&engine),
        settings: &settings,
        scope: &scope,
    };
    let end = agent
        .run(&TurnRequest::new("s1", "Next"), &tx)
        .await
        .unwrap();
    assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));
    assert_eq!(end.steps, 1, "asked once: the summary came first");
    // The oldest exchanges are summed up; the newest prompt is kept as it is.
    let summed = engine.summed.lock().clone();
    assert_eq!(summed.len(), 1);
    let asked = model.asked.lock()[0].clone();
    let first = domain::text_of(&asked.messages[0].blocks);
    assert!(first.starts_with(domain::SUMMARY_HEAD), "{first}");
    assert!(domain::text_of(&asked.messages.last().unwrap().blocks).ends_with("Next"));

    // A model of an unknown window counts 131,072 tokens: 30k fit.
    let model = Scripted::with(vec![Ok(said("Done."))]);
    let engine = Engine::default();
    let kept = Kept {
        turns: Mutex::new(history(3, 40_000)),
    };
    let agent = Agent {
        models: &model,
        transcript: &kept,
        context: Some(&engine),
        ..agent
    };
    agent
        .run(&TurnRequest::new("s1", "Next"), &tx)
        .await
        .unwrap();
    assert!(engine.summed.lock().is_empty());
}
