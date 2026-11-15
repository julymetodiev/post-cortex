// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use post_cortex_proto::pb::SessionInfo;

use crate::InMemoryDocuments;

/// A note: a message of real descriptors, its tag in `description`.
fn note(title: &str, tag: &str) -> SessionInfo {
    SessionInfo {
        name: title.to_string(),
        description: tag.to_string(),
        ..SessionInfo::default()
    }
}

#[tokio::test]
async fn a_kind_is_written_read_listed_and_changed_from_its_version() {
    let docs = InMemoryDocuments::new(Some("ana"), post_cortex_proto::PCX_DESCRIPTOR_SET).unwrap();
    let notes = Typed::new(&docs, "notes/note");

    let first = notes.create(&note("one", "a"), "first").await.unwrap();
    notes.create(&note("two", "b"), "second").await.unwrap();
    assert_eq!(
        (first.document.version, first.document.author.as_deref()),
        (1, Some("ana"))
    );
    let read: Item<SessionInfo> = notes.get(&first.document.id).await.unwrap();
    assert_eq!(read.data, note("one", "a"));

    let tagged: Vec<Item<SessionInfo>> = notes
        .all(Some((&note("", "b"), &["description"])))
        .await
        .unwrap();
    assert_eq!(tagged.len(), 1);
    assert_eq!(tagged[0].document.text, "second");
    assert_eq!(notes.all::<SessionInfo>(None).await.unwrap().len(), 2);

    let id = first.document.id.clone();
    let changed = notes
        .change(&id, 1, &note("one", "c"), "first, again")
        .await
        .unwrap();
    assert_eq!(changed.document.version, 2);
    let stale = notes
        .change(&id, 1, &note("one", "d"), "late")
        .await
        .unwrap_err();
    assert_eq!(stale.kind, ModuleErrorKind::Conflict);
    assert!(stale.message.contains("read version 2"), "{stale:?}");
    // A change that names no version read is refused, not taken for a new document.
    let unread = notes
        .change(&id, 0, &note("one", "e"), "unread")
        .await
        .unwrap_err();
    assert_eq!(unread.kind, ModuleErrorKind::InvalidArgument);
    assert!(
        unread.message.contains("names the version you read"),
        "{unread:?}"
    );
    assert_eq!(
        notes
            .get::<SessionInfo>(&id)
            .await
            .unwrap()
            .document
            .version,
        2
    );

    let missing = notes.get::<SessionInfo>("none").await.unwrap_err();
    assert_eq!(
        (missing.kind, missing.message.as_str()),
        (ModuleErrorKind::NotFound, "no note none")
    );
    // Another kind's documents are not this kind's.
    let other = Typed::new(&docs, "notes/other");
    assert!(other.find::<SessionInfo>(&id).await.unwrap().is_none());
    // Data of another type does not read as this one.
    let wrong = notes.get::<post_cortex_proto::pb::Document>(&id).await;
    assert_eq!(wrong.unwrap_err().kind, ModuleErrorKind::Internal);
}

#[tokio::test]
async fn a_kind_is_read_page_by_page_the_newest_first() {
    let docs = InMemoryDocuments::new(None, post_cortex_proto::PCX_DESCRIPTOR_SET).unwrap();
    let notes = Typed::new(&docs, "notes/note");
    for (title, tag) in [("one", "a"), ("two", "b"), ("three", "a")] {
        notes.create(&note(title, tag), "").await.unwrap();
    }
    let first: Paged<SessionInfo> = notes.page(None, 2, None).await.unwrap();
    let names: Vec<&str> = first.items.iter().map(|i| i.data.name.as_str()).collect();
    assert_eq!(names, ["three", "two"]);
    let rest: Paged<SessionInfo> = notes.page(None, 2, first.next.as_deref()).await.unwrap();
    assert_eq!(rest.items.len(), 1);
    assert_eq!((rest.items[0].data.name.as_str(), rest.next), ("one", None));
    let tagged: Paged<SessionInfo> = notes
        .page(Some((&note("", "a"), &["description"])), 1, None)
        .await
        .unwrap();
    assert_eq!(tagged.items[0].data.name, "three");
    assert!(tagged.next.is_some());
}
