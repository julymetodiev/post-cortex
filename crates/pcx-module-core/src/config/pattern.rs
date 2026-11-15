// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The text a string setting takes (`Setting.pattern`): a regular
//! expression the whole text matches, compiled under the limits a
//! workflow's patterns have (`pcx-expr`, which this crate stays below).

use regex::{Regex, RegexBuilder};

/// Bytes of a pattern at most.
const MAX_PATTERN: usize = 1024;

/// Bytes a pattern's compiled program may take at most.
const PATTERN_SIZE: usize = 1 << 20;

/// `source` compiled to match a whole text.
///
/// # Errors
/// Over 1 KiB, or it does not compile within the size limit; why.
pub fn whole(source: &str) -> Result<Regex, String> {
    if source.len() > MAX_PATTERN {
        return Err(format!("a pattern is at most {MAX_PATTERN} bytes"));
    }
    RegexBuilder::new(&format!("^(?:{source})$"))
        .size_limit(PATTERN_SIZE)
        .build()
        .map_err(|e| format!("its pattern does not compile: {e}"))
}

/// Check `pattern` and `example` as a setting declares them, a string
/// (`string`) or not: a pattern is of a string, compiles and has an example
/// that matches it; there is no example without one.
///
/// # Errors
/// Why the setting is declared wrong.
pub fn declared(pattern: &str, example: &str, string: bool) -> Result<(), String> {
    if pattern.is_empty() {
        return if example.is_empty() {
            Ok(())
        } else {
            Err("an example is of a pattern".to_string())
        };
    }
    if !string {
        return Err("a pattern is of a string".to_string());
    }
    if !whole(pattern)?.is_match(example) {
        return Err(format!(
            "its example \"{example}\" does not match its pattern {pattern}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_matches_a_whole_text() {
        let digits = whole("[0-9]{1,20}").unwrap();
        assert!(digits.is_match("1570896359719047299"));
        assert!(!digits.is_match("id 1570896359719047299"));
        assert!(!digits.is_match(""));
        // An alternative is held whole too.
        let either = whole("a|b").unwrap();
        assert!(either.is_match("a") && !either.is_match("ab"));
        assert!(whole("(").unwrap_err().contains("does not compile"));
        assert!(whole(&"a".repeat(1025)).unwrap_err().contains("at most"));
    }

    #[test]
    fn a_setting_declares_a_pattern_on_a_string_with_an_example_that_matches() {
        declared("", "", false).unwrap();
        declared("[a-z]+", "help", true).unwrap();
        for (pattern, example, string, why) in [
            ("", "help", true, "an example is of a pattern"),
            ("[a-z]+", "help", false, "of a string"),
            ("[a-z]+", "", true, "does not match"),
            ("[a-z]+", "Help", true, "does not match"),
            ("[a-z", "help", true, "does not compile"),
        ] {
            let refused = declared(pattern, example, string).unwrap_err();
            assert!(refused.contains(why), "{pattern} {example}: {refused}");
        }
    }
}
