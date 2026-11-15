// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::tools::Kind;

fn round_trips<T: WireName + PartialEq + std::fmt::Debug>() {
    for v in T::all() {
        assert_eq!(
            lookup::<T>(v.wire()).as_ref(),
            Some(v),
            "wire name of {v:?}"
        );
        assert_eq!(
            lookup::<T>(v.stored()).as_ref(),
            Some(v),
            "stored name of {v:?}"
        );
        // No two variants fold to one name.
        let others = T::all().iter().filter(|o| o != &v);
        for o in others {
            assert_ne!(folded(o.wire()), folded(v.wire()));
            assert_ne!(folded(o.stored()), folded(v.stored()));
        }
    }
}

#[test]
fn every_variant_round_trips() {
    round_trips::<UpdateType>();
    round_trips::<EntityType>();
    round_trips::<RelationType>();
    round_trips::<SessionRole>();
    round_trips::<ContentKind>();
}

/// The spellings the transports accepted before stay accepted: snake case
/// in any case, the stored (Debug) name, and the run-together forms.
#[test]
fn legacy_spellings_are_accepted() {
    assert_eq!(lookup("RequiredBy"), Some(RelationType::RequiredBy));
    assert_eq!(lookup("requiredby"), Some(RelationType::RequiredBy));
    assert_eq!(lookup("codecomponent"), Some(EntityType::CodeComponent));
    assert_eq!(lookup(" Technology "), Some(EntityType::Technology));
    assert_eq!(lookup("code_changed"), Some(UpdateType::CodeChanged));
    assert_eq!(
        lookup("question_answered"),
        Some(UpdateType::QuestionAnswered)
    );
    assert_eq!(lookup("Dependency"), Some(SessionRole::Dependency));
}

/// An unknown name is an invalid argument naming the field and the valid
/// values: never a default.
#[test]
fn an_unknown_name_is_invalid_never_a_default() {
    let err = parse::<UpdateType>("interaction_type", "decison").unwrap_err();
    assert_eq!(err.kind, Kind::InvalidArgument);
    assert_eq!(err.details.parameter.as_deref(), Some("interaction_type"));
    assert!(err.message.contains("decision_made"), "{}", err.message);
    assert!(parse::<SessionRole>("role", "boss").is_err());
    assert!(entities([("x", "weird")], true, Utc::now()).is_err());
    assert_eq!(
        entities([("x", "weird")], false, Utc::now()).unwrap()[0].entity_type,
        EntityType::Concept
    );
    let err = relations([("a", "b", "bogus", "c")]).unwrap_err();
    assert!(
        err.message
            .starts_with("relation[0]: unknown relation_type")
    );
}

#[test]
fn meta_names_the_update_at_fault() {
    let err = update_meta(Some("yesterday"), &[], None, "updates[2].").unwrap_err();
    assert_eq!(
        err.details.parameter.as_deref(),
        Some("updates[2].valid_from")
    );
    let err = update_meta(None, &["x".to_string()], None, "").unwrap_err();
    assert_eq!(err.details.parameter.as_deref(), Some("supersedes"));
    let meta = update_meta(Some("2026-01-02"), &[], Some(0.5), "").unwrap();
    assert!(meta.valid_from.is_some());
}

#[test]
fn sources_modes_and_dates_are_strict() {
    assert_eq!(sources(None).unwrap(), Corpus::default());
    assert!(sources(Some("everything")).is_err());
    assert!(mode(Some("slow")).is_err());
    assert!(mode(None).unwrap().is_none());
    assert!(date_range(Some("2024-01-01T00:00:00Z"), None).is_err());
    assert!(date_range(Some("x"), Some("2024-01-01T00:00:00Z")).is_err());
}
