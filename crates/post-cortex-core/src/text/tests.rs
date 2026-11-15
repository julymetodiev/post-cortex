// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn splits_any_script_on_punctuation() {
    assert_eq!(
        tokens("What did Caroline say?").join(" "),
        "what did caroline say"
    );
    assert_eq!(
        tokens("Какво решихме за базата?").join(" "),
        "какво решихме за базата"
    );
    assert_eq!(
        tokens("Přišel jsem, viděl jsem").join(" "),
        "přišel jsem viděl jsem"
    );
    assert!(tokens("?!").is_empty());
}

#[test]
fn a_short_text_is_shown_whole() {
    assert_eq!(passage("We deploy with pcx-swap.", "deploy", 400), None);
}

#[test]
fn a_long_text_shows_the_window_with_the_most_query_words() {
    let filler = "lorem ipsum dolor sit amet ".repeat(40);
    let text = format!("{filler}The board decided to deploy with pcx-swap on Friday. {filler}");
    let shown = passage(&text, "When do we deploy with pcx-swap?", 120).unwrap();
    assert!(shown.contains("deploy with pcx-swap on Friday"), "{shown}");
    assert!(shown.starts_with('…') && shown.ends_with('…'), "{shown}");
    assert!(
        shown.chars().count() <= 122,
        "{} chars",
        shown.chars().count()
    );
}

#[test]
fn without_a_query_word_the_passage_is_the_start() {
    let text = "alpha beta gamma delta ".repeat(30);
    let shown = passage(&text, "zeta", 50).unwrap();
    assert!(shown.starts_with("alpha beta"), "{shown}");
    assert!(shown.ends_with('…'));
}

#[test]
fn a_passage_is_found_in_any_script() {
    let text = format!(
        "{} Решихме базата да е SurrealDB. {}",
        "текст ".repeat(60),
        "още ".repeat(60)
    );
    let shown = passage(&text, "Какво решихме за базата?", 80).unwrap();
    assert!(shown.contains("Решихме базата да е SurrealDB"), "{shown}");
}
