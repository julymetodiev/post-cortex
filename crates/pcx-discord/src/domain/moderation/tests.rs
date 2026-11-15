// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use chrono::TimeZone;

use super::*;
use crate::domain::tests::config;

fn moderating(ban: bool) -> DiscordConfig {
    DiscordConfig {
        moderation: Some(Moderation {
            longest_timeout_minutes: 60,
            ban,
        }),
        ..config()
    }
}

#[test]
fn a_timeout_ends_its_minutes_later_and_zero_lifts_it() {
    let now = Utc.with_ymd_and_hms(2026, 11, 14, 9, 30, 15).unwrap();
    let moderated = moderating(false);
    assert_eq!(
        until(&moderated, now, 60),
        Ok(Some("2026-11-14T10:30:15Z".to_string()))
    );
    assert_eq!(until(&moderated, now, 0), Ok(None));
    assert!(matches!(
        until(&moderated, now, 61),
        Err(Refused::Request(_))
    ));
    assert!(matches!(
        until(&config(), now, 1),
        Err(Refused::Moderation(_))
    ));
}

#[test]
fn a_ban_is_refused_unless_the_settings_let_it_and_deletes_at_most_a_week() {
    assert_eq!(ban(&moderating(true), 24), Ok(86_400));
    assert_eq!(ban(&moderating(true), 0), Ok(0));
    assert!(matches!(
        ban(&moderating(true), LONGEST_DELETE_HOURS + 1),
        Err(Refused::Request(_))
    ));
    assert!(matches!(
        ban(&moderating(false), 1),
        Err(Refused::Moderation(_))
    ));
    assert!(matches!(ban(&config(), 1), Err(Refused::Moderation(_))));
}

#[test]
fn the_longest_timeout_is_one_minute_to_discords_28_days() {
    for (minutes, ok) in [
        (1, true),
        (LONGEST_TIMEOUT_MINUTES, true),
        (0, false),
        (LONGEST_TIMEOUT_MINUTES + 1, false),
    ] {
        let moderation = Moderation {
            longest_timeout_minutes: minutes,
            ban: false,
        };
        assert_eq!(check(&moderation).is_ok(), ok, "{minutes}");
    }
}
