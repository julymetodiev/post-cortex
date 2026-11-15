// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

pub fn config() -> DiscordConfig {
    let channel = |name: &str, id: &str, post: bool, hear: bool| Channel {
        name: name.to_string(),
        channel_id: id.to_string(),
        post,
        hear,
    };
    DiscordConfig {
        bot_token: "token".to_string(),
        guild_id: "100".to_string(),
        workspace_id: "public".to_string(),
        channels: vec![
            channel("help", "200", true, true),
            channel("general", "201", false, true),
            channel("changelog", "202", true, false),
        ],
        roles: vec!["Member".to_string()],
        tags: vec!["answered".to_string(), "needs-team".to_string()],
        moderation: None,
    }
}

fn field_of(refused: Result<(), Refused>) -> String {
    match refused {
        Err(Refused::Setting { field, .. }) => field,
        other => panic!("not a wrong setting: {other:?}"),
    }
}

#[test]
fn settings_name_a_token_a_server_a_workspace_and_each_channel_once() {
    assert_eq!(check(&config()), Ok(()));
    let with = |change: fn(&mut DiscordConfig)| {
        let mut changed = config();
        change(&mut changed);
        field_of(check(&changed))
    };
    assert_eq!(with(|c| c.bot_token = " ".into()), "bot_token");
    assert_eq!(with(|c| c.guild_id = "PCX".into()), "guild_id");
    assert_eq!(with(|c| c.workspace_id.clear()), "workspace_id");
    assert_eq!(
        with(|c| c.channels[1].name = "General".into()),
        "channels.1.name"
    );
    assert_eq!(
        with(|c| c.channels[1].name = "help".into()),
        "channels.1.name"
    );
    assert_eq!(
        with(|c| c.channels[2].channel_id = "#changelog".into()),
        "channels.2.channel_id"
    );
    assert_eq!(
        with(|c| c.channels[2].channel_id = "200".into()),
        "channels.2.channel_id"
    );
    assert_eq!(with(|c| c.roles.push("Member".into())), "roles.1");
    assert_eq!(with(|c| c.tags.push(String::new())), "tags.2");
}

#[test]
fn the_bot_posts_only_where_its_settings_say() {
    let config = config();
    assert_eq!(destination(&config, "help").unwrap().channel_id, "200");
    assert_eq!(destination(&config, "changelog").unwrap().channel_id, "202");
    // It hears `general` and does not post there; `ops` is not named.
    for name in ["general", "ops", ""] {
        assert_eq!(
            destination(&config, name),
            Err(Refused::Channel(name.to_string()))
        );
    }
    assert_eq!(channel(&config, "201").unwrap().name, "general");
    assert!(channel(&config, "999").is_none());
}

#[test]
fn an_id_is_digits_and_a_message_says_something() {
    assert!(is_id("1570896359719047299"));
    for bad in ["", "12a", "1/2", "123456789012345678901", "-1"] {
        assert!(!is_id(bad), "{bad}");
    }
    assert!(id("thread_id", "../guilds/1").is_err());
    assert_eq!(said("  hi \n"), Ok("hi"));
    assert!(said(" \n").is_err());
}

#[test]
fn an_embed_has_something_within_discords_most_and_an_https_link() {
    let card_of = |title: &str, description: String, url: &str| Embed {
        title: title.to_string(),
        description,
        url: url.to_string(),
        ..Embed::default()
    };
    assert_eq!(
        card(&card_of("0.5.1", "Fixes".into(), "https://pcx.is")),
        Ok(())
    );
    assert!(card(&card_of("", String::new(), "")).is_err());
    assert!(card(&card_of("t", "x".repeat(4097), "")).is_err());
    assert!(card(&card_of(&"t".repeat(257), String::new(), "")).is_err());
    assert!(card(&card_of("t", String::new(), "javascript:alert(1)")).is_err());
}

#[test]
fn an_embeds_pictures_go_as_cards_of_its_link() {
    let with = |url: &str, images: &[&str]| Embed {
        title: "0.6.0".to_string(),
        url: url.to_string(),
        images: images.iter().map(|i| (*i).to_string()).collect(),
        // What a caller sets as Discord's own form is not sent.
        image: Some(EmbedImage {
            url: "https://elsewhere.example/x.png".to_string(),
        }),
        ..Embed::default()
    };
    let picture = |url: &str| {
        Some(EmbedImage {
            url: url.to_string(),
        })
    };
    let (a, b) = ("https://docs.pcx.is/a.png", "https://docs.pcx.is/b.png");

    let none = cards(with("", &[])).unwrap();
    assert_eq!(none.len(), 1);
    assert_eq!(none[0].image, None);

    let one = cards(with("", &[a])).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!((one[0].image.clone(), one[0].images.len()), (picture(a), 0));

    let two = cards(with("https://pcx.is", &[a, b])).unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].title, "0.6.0");
    assert_eq!(two[0].image, picture(a));
    assert_eq!(
        two[1],
        Embed {
            url: "https://pcx.is".to_string(),
            image: picture(b),
            ..Embed::default()
        }
    );

    assert!(cards(with("", &[a, b])).is_err(), "several need a link");
    assert!(cards(with("https://pcx.is", &["http://x/a.png"])).is_err());
    assert!(cards(with("https://pcx.is", &[a, a, a, a, a])).is_err());
}

#[test]
fn a_long_text_is_cut_at_line_ends_into_messages_that_fit() {
    assert_eq!(pieces("short"), ["short"]);
    let line = "a".repeat(999);
    let text = format!("{line}\n{line}\n{line}");
    let cut = pieces(&text);
    assert_eq!(cut.len(), 2);
    assert_eq!(cut[0], format!("{line}\n{line}"));
    assert_eq!(cut[1], line);

    // No line end, no space: cut where it fits, on a character.
    let solid = "ж".repeat(4500);
    let cut = pieces(&solid);
    assert_eq!(
        cut.iter().map(|p| p.chars().count()).collect::<Vec<_>>(),
        [2000, 2000, 500]
    );
    assert_eq!(cut.concat(), solid);
    assert!(pieces("").is_empty());
}
