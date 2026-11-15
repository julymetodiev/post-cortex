// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

// What a setting's text must be (docs/host.md, what a text must be): the
// check of a configuration against its settings' patterns.

use super::*;

#[test]
fn a_setting_with_a_pattern_takes_only_text_that_matches_it_whole() {
    // Discord's settings: ids of digits, a channel's name, roles and tags
    // each its own value (prod 2026-11-14: two tags typed on one line with
    // a comma were saved, and found wrong only by the running module).
    let manifest = configured(
        post_cortex_proto::DISCORD_DESCRIPTOR_SET,
        "pcx.discord.v1.DiscordConfig",
    );
    let discord = ConfigType::of(&manifest).unwrap().unwrap();
    type Change = fn(&mut Value);
    let read = |change: Change| {
        let mut config = json!({
            "bot_token": "k",
            "guild_id": "1570896359719047299",
            "workspace_id": WORKSPACE,
            "channels": [{ "name": "help", "channel_id": "1570896788792279173", "post": true }],
            "roles": ["Member"],
            "tags": ["answered", "needs-team"],
        });
        change(&mut config);
        discord.check(&discord.from_json(&config).unwrap())
    };
    read(|_| {}).unwrap();
    let cases: [(Change, &str, &str); 4] = [
        (
            |c| c["guild_id"] = json!("PCX"),
            "guild_id",
            "1570896359719047299",
        ),
        (
            |c| c["tags"] = json!(["answered, needs-team"]),
            "tags[0]",
            "answered",
        ),
        (
            |c| c["channels"][0]["name"] = json!("Help Forum"),
            "channels[0].name",
            "help",
        ),
        (
            |c| c["channels"][0]["channel_id"] = json!("#help"),
            "channels[0].channel_id",
            "1570896788792279173",
        ),
    ];
    for (change, field, example) in cases {
        let refused = read(change).unwrap_err();
        assert_eq!(refused.field, field);
        assert!(
            refused.reason.starts_with(&format!("is like {example} ")),
            "{field}: {}",
            refused.reason
        );
    }
}
