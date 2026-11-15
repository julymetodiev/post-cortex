// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use pcx_team::Stakes;

fn todo_board() -> Board {
    board(
        " Sprint ",
        vec!["todo".into(), " doing ".into(), "done".into()],
        vec![],
        None,
        "s".into(),
    )
    .unwrap()
}

#[test]
fn a_board_has_a_name_and_its_columns_once_each() {
    let b = todo_board();
    assert_eq!(b.name, "Sprint");
    assert_eq!(b.columns, ["todo", "doing", "done"]);
    assert!(matches!(
        board("x", vec![], vec![], None, "s".into()),
        Err(Broken::Columns(_))
    ));
    assert!(matches!(
        board("x", vec!["a".into(), "a".into()], vec![], None, "s".into()),
        Err(Broken::Columns(_))
    ));
    assert_eq!(
        board(" ", vec!["a".into()], vec![], None, "s".into()),
        Err(Broken::Empty("name"))
    );
}

#[test]
fn a_task_goes_to_a_column_of_its_board() {
    let b = todo_board();
    assert_eq!(column(&b, None).unwrap(), "todo");
    assert_eq!(column(&b, Some("done")).unwrap(), "done");
    assert!(matches!(
        column(&b, Some("later")),
        Err(Broken::UnknownColumn { .. })
    ));
}

#[test]
fn a_risky_decision_holds_its_task_until_approved() {
    let safe = decide(
        "t",
        "use sqlite",
        "small",
        "ana",
        Risk::None,
        Stakes::default(),
    )
    .unwrap();
    assert!(!safe.pending());
    let risky = decide(
        "t",
        "drop the table",
        "unused",
        "ana",
        Risk::Data,
        Stakes::default(),
    )
    .unwrap();
    assert!(risky.pending());
    assert_eq!(
        may_move([("d1", &safe), ("d2", &risky)]),
        Err(Broken::Held {
            decision: "d2".into()
        })
    );
    let approved = approve(risky, "julius", Some(" ok ".into())).unwrap();
    assert_eq!(approved.approved_by.as_deref(), Some("julius"));
    assert_eq!(approved.approval_note.as_deref(), Some("ok"));
    assert_eq!(revoke(approved.clone()).unwrap().approval_note, None);
    assert_eq!(may_move([("d2", &approved)]), Ok(()));
    assert_eq!(approve(approved, "julius", None), Err(Broken::NotPending));
    assert_eq!(approve(safe, "julius", None), Err(Broken::NotPending));
}

#[test]
fn only_its_author_withdraws_a_waiting_decision_and_it_holds_nothing_after() {
    let risky = decide(
        "t",
        "deploy",
        "asked",
        "workflow:check",
        Risk::Code,
        Stakes::default(),
    )
    .unwrap();
    assert_eq!(
        withdraw(risky.clone(), "ana", "its run ended"),
        Err(Broken::NotTheDecider {
            decider: "workflow:check".into()
        })
    );
    assert_eq!(
        withdraw(risky.clone(), "workflow:check", " "),
        Err(Broken::Empty("why"))
    );
    let withdrawn = withdraw(risky, "workflow:check", "its run ended").unwrap();
    assert_eq!(withdrawn.withdrawn.as_deref(), Some("its run ended"));
    assert!(!withdrawn.pending());
    assert_eq!(may_move([("d", &withdrawn)]), Ok(()));
    assert_eq!(
        approve(withdrawn.clone(), "julius", None),
        Err(Broken::NotPending)
    );
    assert_eq!(
        withdraw(withdrawn, "workflow:check", "again"),
        Err(Broken::NotPending)
    );
}

#[test]
fn a_declined_decision_waits_for_no_one_and_holds_nothing() {
    let risky = decide(
        "t",
        "restart it",
        "it loops",
        "eq-medic",
        Risk::Operations,
        Stakes::default(),
    )
    .unwrap();
    let declined = decline(risky, "julius", Some(" not now ".into())).unwrap();
    assert_eq!(declined.declined_by.as_deref(), Some("julius"));
    assert_eq!(declined.decline_note.as_deref(), Some("not now"));
    assert!(!declined.pending());
    assert_eq!(may_move([("d", &declined)]), Ok(()));
    assert_eq!(
        approve(declined.clone(), "julius", None),
        Err(Broken::NotPending)
    );
    assert_eq!(decline(declined, "ana", None), Err(Broken::NotPending));
}

#[test]
fn a_comment_is_titled_by_its_first_line() {
    let c = comment("t", "first line\nmore", None, Vec::new()).unwrap();
    assert_eq!(c.title, "first line");
    assert_eq!(
        comment("t", "  ", None, Vec::new()),
        Err(Broken::Empty("text"))
    );
    assert_eq!(
        decide("t", "x", " ", "ana", Risk::None, Stakes::default()),
        Err(Broken::Empty("why"))
    );
}

#[test]
fn a_column_has_a_stage_and_a_board_without_stages_reads_its_last_as_done() {
    let b = todo_board();
    assert_eq!(
        b.stages,
        [Stage::Open as i32, Stage::Open as i32, Stage::Done as i32]
    );
    let before = Board {
        stages: vec![],
        ..todo_board()
    };
    assert_eq!(stage(&before, "doing"), Stage::Open);
    assert_eq!(stage(&before, "done"), Stage::Done);
    assert!(closes(&before, "done") && !closes(&before, "todo"));
    assert_eq!(staged(before).stages, todo_board().stages);

    let staged = board(
        "Ops",
        vec!["todo".into(), "done".into(), "dropped".into()],
        vec![
            Stage::Open.into(),
            Stage::Done.into(),
            Stage::Canceled.into(),
        ],
        None,
        "s".into(),
    )
    .unwrap();
    assert_eq!(stage(&staged, "done"), Stage::Done);
    assert_eq!(stage(&staged, "dropped"), Stage::Canceled);
    assert!(closes(&staged, "dropped"), "canceling closes a task");

    for stages in [
        vec![Stage::Open.into()],
        vec![0, Stage::Done.into()],
        vec![9, 2],
    ] {
        assert!(matches!(
            board("x", vec!["a".into(), "b".into()], stages, None, "s".into()),
            Err(Broken::Columns(_))
        ));
    }
}

#[test]
fn a_task_is_named_by_its_id_or_a_short_id_that_begins_one() {
    let ids = ["273b0c33-d61a", "273b0c99-0000", "ac1cea44-698a"];
    let named = |name| task_id(name, ids.iter().copied());
    assert_eq!(named("ac1cea44-698a"), Ok(Some("ac1cea44-698a".into())));
    assert_eq!(named("ac1cea44"), Ok(Some("ac1cea44-698a".into())));
    // Fewer characters than a short id has, or the start of none.
    assert_eq!(named("ac1cea4"), Ok(None));
    assert_eq!(named("ffffffff"), Ok(None));
    assert_eq!(named("273b0c"), Ok(None), "too short to name one of two");
    assert_eq!(task_id("273b0c3", ["273b0c33-d61a"].into_iter()), Ok(None));
    let both = ["273b0c33-d61a", "273b0c33-ffff"];
    assert_eq!(
        task_id("273b0c33", both.iter().copied()),
        Err(Broken::Ambiguous {
            short: "273b0c33".into(),
            tasks: vec!["273b0c33-d61a".into(), "273b0c33-ffff".into()],
        })
    );
    assert_eq!(full("ffffffff", ids.iter().copied()), Ok("ffffffff".into()));
}
