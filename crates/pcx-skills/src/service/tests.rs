// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The skills service over documents held in memory.

use super::*;

use pcx_documents::InMemoryDocuments;

#[tokio::test]
async fn skills_are_added_once_by_name_listed_and_changed_from_their_version() {
    let docs =
        InMemoryDocuments::new(Some("ana"), post_cortex_proto::SKILLS_DESCRIPTOR_SET).unwrap();
    let library = Service::new(&docs);
    let added = library
        .add(
            "review",
            "Review a change",
            "Read the diff.",
            vec!["git_diff".into()],
        )
        .await
        .unwrap();
    library
        .add("plan", "Plan the work", "Think first.", Vec::new())
        .await
        .unwrap();
    let taken = library
        .add("review", "Again", "Twice.", Vec::new())
        .await
        .unwrap_err();
    assert_eq!(taken.kind, ModuleErrorKind::Conflict);
    let unnamed = library
        .add("Review Me", "d", "i", Vec::new())
        .await
        .unwrap_err();
    assert_eq!(unnamed.kind, ModuleErrorKind::InvalidArgument);

    let names: Vec<String> = library
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.data.name)
        .collect();
    assert_eq!(names, ["plan", "review"]);

    let changed = library
        .update(
            "review",
            added.document.version,
            Revision {
                instructions: Some("Read the diff, then the tests.".into()),
                ..Revision::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (changed.document.version, changed.document.text.as_str()),
        (2, "Read the diff, then the tests.")
    );
    let stale = library
        .update("review", added.document.version, Revision::default())
        .await
        .unwrap_err();
    assert_eq!(stale.kind, ModuleErrorKind::Conflict);

    let prompt = library.prompt("review").await.unwrap();
    assert_eq!(
        prompt,
        SkillPrompt {
            description: "Review a change".into(),
            text: "Read the diff, then the tests.\n\nTools this skill needs: git_diff.".into(),
        }
    );
    let missing = library.prompt("ship").await.unwrap_err();
    assert_eq!(missing.kind, ModuleErrorKind::NotFound);
}

#[tokio::test]
async fn the_built_in_skills_are_added_once_and_a_persons_change_stays() {
    let module =
        InMemoryDocuments::new(Some("skills"), post_cortex_proto::SKILLS_DESCRIPTOR_SET).unwrap();
    let docs = module.as_author("ana");
    let library = Service::new(&docs);
    let seeding = Service::new(&module);
    assert_eq!(
        seeding.seed().await.unwrap(),
        ["workflow-builder", "module-builder"]
    );
    let builder = library.get("workflow-builder").await.unwrap();
    assert!(builder.data.tools.contains(&"workflow_dry_run".to_string()));
    assert!(builder.document.text.starts_with("# Building workflows"));
    library
        .update(
            "workflow-builder",
            builder.document.version,
            Revision {
                instructions: Some("Ours.".into()),
                ..Revision::default()
            },
        )
        .await
        .unwrap();
    assert!(
        seeding.seed().await.unwrap().is_empty(),
        "a person's change stays"
    );
    assert_eq!(
        library.get("workflow-builder").await.unwrap().document.text,
        "Ours."
    );
}

#[tokio::test]
async fn a_built_in_skill_the_module_last_wrote_is_renewed_when_the_text_shipped_changes() {
    let module =
        InMemoryDocuments::new(Some("skills"), post_cortex_proto::SKILLS_DESCRIPTOR_SET).unwrap();
    let seeding = Service::new(&module);
    seeding.seed().await.unwrap();
    // An older text, as an earlier release of the module wrote it.
    let shipped = seeding.get("workflow-builder").await.unwrap();
    seeding
        .update(
            "workflow-builder",
            shipped.document.version,
            Revision {
                instructions: Some("An older text.".into()),
                ..Revision::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        seeding.seed().await.unwrap(),
        ["workflow-builder"],
        "renewed"
    );
    let renewed = seeding.get("workflow-builder").await.unwrap();
    assert!(renewed.document.text.starts_with("# Building workflows"));
    assert!(
        seeding.seed().await.unwrap().is_empty(),
        "nothing to renew once it is the text shipped"
    );
}
