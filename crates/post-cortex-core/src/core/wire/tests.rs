use crate::core::content::ContentKind;
use crate::core::context_update::{EntityType, RelationType, UpdateType};
use crate::workspace::SessionRole;
use std::fmt::Debug;
use std::str::FromStr;

/// Every variant: `as_str` parses back, the stored (serde) name parses as
/// a legacy alias and is what serde writes, and nothing else parses.
fn round_trips<T>(all: &[T], as_str: fn(&T) -> &'static str, stored: fn(&T) -> &'static str)
where
    T: FromStr<Err = super::UnknownVariant> + PartialEq + Debug + serde::Serialize,
{
    let mut wire_names = std::collections::HashSet::new();
    for v in all {
        assert_eq!(T::from_str(as_str(v)).as_ref(), Ok(v));
        assert_eq!(T::from_str(stored(v)).as_ref(), Ok(v));
        assert_eq!(serde_json::to_value(v).unwrap(), stored(v));
        assert!(wire_names.insert(as_str(v)), "duplicate wire name");
    }
    for bad in ["", "unknown", " qa", "DEPENDS_ON"] {
        let err = T::from_str(bad).unwrap_err();
        assert_eq!(err.value, bad);
    }
}

#[test]
fn update_type_names_round_trip() {
    round_trips(UpdateType::ALL, UpdateType::as_str, UpdateType::stored_name);
    assert_eq!(UpdateType::CodeChanged.as_str(), "code_change");
}

#[test]
fn entity_type_names_round_trip() {
    round_trips(EntityType::ALL, EntityType::as_str, EntityType::stored_name);
}

#[test]
fn relation_type_names_round_trip() {
    round_trips(
        RelationType::ALL,
        RelationType::as_str,
        RelationType::stored_name,
    );
}

#[test]
fn session_role_names_round_trip() {
    round_trips(
        SessionRole::ALL,
        SessionRole::as_str,
        SessionRole::stored_name,
    );
    assert_eq!(SessionRole::Dependency.to_string(), "dependency");
}

#[test]
fn content_kind_names_round_trip() {
    round_trips(
        ContentKind::ALL,
        ContentKind::as_str,
        ContentKind::stored_name,
    );
    let records: Vec<_> = ContentKind::ALL.iter().filter(|k| !k.is_record()).collect();
    assert_eq!(
        records,
        [
            &ContentKind::EntityDescription,
            &ContentKind::Message,
            &ContentKind::Document
        ]
    );
}

#[test]
fn unknown_names_say_what_was_expected() {
    let err = "depends-on".parse::<RelationType>().unwrap_err();
    assert_eq!(err.to_string(), "unknown relation type: \"depends-on\"");
}
