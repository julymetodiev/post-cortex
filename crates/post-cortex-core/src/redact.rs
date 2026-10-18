// Copyright (c) 2025 Julius ML
// MIT License

//! Secret and personal-data redaction applied on the canonical write path.
//!
//! Every transport reaches storage through `update_context`, so redacting
//! there covers MCP, gRPC and future REST writes alike. Matches are replaced
//! with `[REDACTED:<kind>]`; the replacement is deterministic, so an entity
//! name and the relations that point at it stay consistent after redaction.
//!
//! Kept: UUIDs, host/pod/node names, private and tailnet addresses — they are
//! pseudonymous and needed to correlate incidents. Removed: credentials,
//! e-mail addresses, phone numbers and public IPv4 addresses.

use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::LazyLock;

use crate::core::context_update::{EntityRelationship, UpdateContent};
use crate::services::UpdateContextRequest;

/// Redaction settings carried by the system config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionConfig {
    /// Apply redaction on every write (default: true).
    pub enabled: bool,
    /// Extra regexes; each match becomes `[REDACTED:custom]`.
    #[serde(default)]
    pub extra_patterns: Vec<String>,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_patterns: Vec::new(),
        }
    }
}

/// How a built-in pattern rewrites its match.
enum Replace {
    /// The whole match becomes `[REDACTED:kind]`.
    Whole,
    /// Keep capture group 1 (and group 2, if any, after the marker),
    /// redact the rest (e.g. `password: ` + value).
    KeepPrefix,
    /// Public IPv4 only: private, loopback and tailnet ranges are kept.
    PublicIpv4,
}

struct Rule {
    kind: &'static str,
    re: Regex,
    replace: Replace,
}

fn rule(kind: &'static str, pattern: &str, replace: Replace) -> Rule {
    Rule {
        kind,
        re: Regex::new(pattern).expect("built-in redaction pattern must compile"),
        replace,
    }
}

// Order matters: specific credential shapes run before the generic
// `key = value` rule, and URLs with credentials before e-mail addresses
// (`user:pass@host` would otherwise read as an address).
static BUILTIN: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule(
            "private_key",
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
            Replace::Whole,
        ),
        rule(
            "jwt",
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            Replace::Whole,
        ),
        rule(
            "api_key",
            r"\b(?:sk-ant-[A-Za-z0-9_-]{20,}|sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{40,}|glpat-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|xox[abps]-[A-Za-z0-9-]{10,}|AIza[0-9A-Za-z_-]{35})",
            Replace::Whole,
        ),
        rule(
            "telegram_token",
            r"\b\d{8,10}:[A-Za-z0-9_-]{35}\b",
            Replace::Whole,
        ),
        rule(
            "bearer",
            r"(?i)(\bbearer\s+)[A-Za-z0-9._~+/=-]{16,}",
            Replace::KeepPrefix,
        ),
        rule(
            "url_password",
            r"(?i)(\b[a-z][a-z0-9+.-]*://[^\s:/@]+:)[^\s@/]+(@)",
            Replace::KeepPrefix,
        ),
        rule(
            "secret",
            r#"(?i)(\b(?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)["']?\s*[:=]\s*["']?)[^\s"',;]{4,}"#,
            Replace::KeepPrefix,
        ),
        rule(
            "email",
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
            Replace::Whole,
        ),
        rule(
            "phone",
            r"(?:\+\d{1,3}[\s-]?\(?\d{1,4}\)?(?:[\s-]?\d{2,4}){2,4}|\b0[89]\d[\s-]?\d{3}[\s-]?\d{3,4}\b)",
            Replace::Whole,
        ),
        rule("ip", r"\b(?:\d{1,3}\.){3}\d{1,3}\b", Replace::PublicIpv4),
    ]
});

fn is_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    let tailnet = o[0] == 100 && (64..128).contains(&o[1]);
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || tailnet)
}

/// Redacts text with the built-in rules plus configured extras.
pub struct Redactor {
    enabled: bool,
    extra: Vec<Regex>,
}

/// Per-kind count of replacements made by one redaction pass.
pub type RedactionCounts = BTreeMap<&'static str, usize>;

impl Redactor {
    /// Build from config. Invalid extra patterns are skipped and returned
    /// so the caller can log them; they never disable the built-in rules.
    #[must_use]
    pub fn new(config: &RedactionConfig) -> (Self, Vec<String>) {
        let mut extra = Vec::new();
        let mut invalid = Vec::new();
        for p in &config.extra_patterns {
            match Regex::new(p) {
                Ok(re) => extra.push(re),
                Err(e) => invalid.push(format!("{p}: {e}")),
            }
        }
        (
            Self {
                enabled: config.enabled,
                extra,
            },
            invalid,
        )
    }

    /// Redactor that changes nothing.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            extra: Vec::new(),
        }
    }

    /// Redact one string, adding replacement counts to `counts`.
    pub fn redact_str(&self, text: &str, counts: &mut RedactionCounts) -> String {
        if !self.enabled || text.is_empty() {
            return text.to_string();
        }
        let mut out = text.to_string();
        for r in BUILTIN.iter() {
            let mut n = 0;
            let replaced = r.re.replace_all(&out, |c: &Captures| {
                let whole = &c[0];
                match r.replace {
                    Replace::Whole => {
                        n += 1;
                        format!("[REDACTED:{}]", r.kind)
                    }
                    Replace::KeepPrefix => {
                        n += 1;
                        let suffix = c.get(2).map_or("", |m| m.as_str());
                        format!("{}[REDACTED:{}]{suffix}", &c[1], r.kind)
                    }
                    Replace::PublicIpv4 => match whole.parse::<Ipv4Addr>() {
                        Ok(ip) if is_public(ip) => {
                            n += 1;
                            format!("[REDACTED:{}]", r.kind)
                        }
                        _ => whole.to_string(),
                    },
                }
            });
            if n > 0 {
                out = replaced.into_owned();
                *counts.entry(r.kind).or_default() += n;
            }
        }
        for re in &self.extra {
            let n = re.find_iter(&out).count();
            if n > 0 {
                out = re.replace_all(&out, "[REDACTED:custom]").into_owned();
                *counts.entry("custom").or_default() += n;
            }
        }
        out
    }

    fn redact_vec(&self, items: &mut [String], counts: &mut RedactionCounts) {
        for s in items {
            *s = self.redact_str(s, counts);
        }
    }

    fn redact_content(&self, c: &mut UpdateContent, counts: &mut RedactionCounts) {
        c.title = self.redact_str(&c.title, counts);
        c.description = self.redact_str(&c.description, counts);
        self.redact_vec(&mut c.details, counts);
        self.redact_vec(&mut c.examples, counts);
        self.redact_vec(&mut c.implications, counts);
    }

    fn redact_relation(&self, r: &mut EntityRelationship, counts: &mut RedactionCounts) {
        r.from_entity = self.redact_str(&r.from_entity, counts);
        r.to_entity = self.redact_str(&r.to_entity, counts);
        r.context = self.redact_str(&r.context, counts);
    }

    /// Redact every free-text field of a write request in place.
    pub fn redact_request(&self, req: &mut UpdateContextRequest) -> RedactionCounts {
        let mut counts = RedactionCounts::new();
        if !self.enabled {
            return counts;
        }
        self.redact_content(&mut req.content, &mut counts);
        for e in &mut req.entities {
            e.name = self.redact_str(&e.name, &mut counts);
        }
        for r in &mut req.relations {
            self.redact_relation(r, &mut counts);
        }
        if let Some(code) = &mut req.code_reference {
            code.code_snippet = self.redact_str(&code.code_snippet, &mut counts);
            code.change_description = self.redact_str(&code.change_description, &mut counts);
        }
        counts
    }

    /// Redact every string inside a JSON document (used for imports, whose
    /// records bypass `update_context`). Object keys are left untouched.
    pub fn redact_json(&self, value: &mut serde_json::Value, counts: &mut RedactionCounts) {
        match value {
            serde_json::Value::String(s) => *s = self.redact_str(s, counts),
            serde_json::Value::Array(items) => {
                for v in items {
                    self.redact_json(v, counts);
                }
            }
            serde_json::Value::Object(map) => {
                for v in map.values_mut() {
                    self.redact_json(v, counts);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red(text: &str) -> (String, RedactionCounts) {
        let (r, _) = Redactor::new(&RedactionConfig::default());
        let mut counts = RedactionCounts::new();
        let out = r.redact_str(text, &mut counts);
        (out, counts)
    }

    #[test]
    fn credentials_are_redacted() {
        let (out, c) = red(
            "Bearer abcdefghijklmnopqrstuvwxyz012345 and password: hunter2pass and \
             postgres://app:not-a-real-pass@db.example:5432/app",
        );
        assert!(!out.contains("abcdefghijklmnop"));
        assert!(!out.contains("hunter2pass"));
        assert!(!out.contains("not-a-real-pass"));
        assert!(out.contains("Bearer [REDACTED:bearer]"));
        assert!(
            out.contains("postgres://app:[REDACTED:url_password]@db.example"),
            "{out}"
        );
        assert_eq!(c["url_password"], 1);
    }

    #[test]
    fn tokens_and_keys_are_redacted() {
        // Fake values are assembled at run time so secret scanners don't
        // flag the test source as a leaked credential.
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyMTIzIn0.c2lnbmF0dXJlLXZhbHVl";
        let api_key = format!("sk-ant-{}", "x".repeat(24));
        let bot = format!("{}:{}", "1".repeat(10), "x".repeat(35));
        let (out, c) = red(&format!("jwt {jwt} key {api_key} bot {bot}"));
        assert!(!out.contains(jwt), "{out}");
        assert_eq!(c["jwt"], 1);
        assert_eq!(c["api_key"], 1);
        assert_eq!(c["telegram_token"], 1);
    }

    #[test]
    fn personal_data_is_redacted() {
        let (out, c) = red("mail julius@example.com, tel +359 88 123 4567 or 0881234567");
        assert_eq!(out.matches("[REDACTED:email]").count(), 1);
        assert_eq!(c["phone"], 2);
    }

    #[test]
    fn private_and_tailnet_ips_are_kept() {
        let (out, c) =
            red("tailnet 100.64.0.7, node 10.0.0.5, lan 192.168.0.10, client 203.0.113.7");
        assert!(out.contains("100.64.0.7"));
        assert!(out.contains("10.0.0.5"));
        assert!(out.contains("192.168.0.10"));
        assert!(!out.contains("203.0.113.7"));
        assert_eq!(c["ip"], 1);
    }

    #[test]
    fn ordinary_text_is_untouched() {
        let text = "svc-social v0.1.34, card t_850006da, user c103a600-836a-4ed0-be74-18b58d0b055e, token_budget = 4000";
        let (out, c) = red(text);
        assert_eq!(out, text);
        assert!(c.is_empty());
    }

    #[test]
    fn disabled_and_custom_patterns() {
        let (off, _) = Redactor::new(&RedactionConfig {
            enabled: false,
            extra_patterns: vec![],
        });
        let mut c = RedactionCounts::new();
        assert_eq!(off.redact_str("a@b.co", &mut c), "a@b.co");

        let (on, invalid) = Redactor::new(&RedactionConfig {
            enabled: true,
            extra_patterns: vec![r"ORD-\d{6}".into(), "(".into()],
        });
        assert_eq!(invalid.len(), 1);
        assert_eq!(
            on.redact_str("order ORD-123456", &mut c),
            "order [REDACTED:custom]"
        );
    }

    #[test]
    fn json_strings_are_redacted_keys_kept() {
        let (r, _) = Redactor::new(&RedactionConfig::default());
        let mut v = serde_json::json!({"email": "a@b.co", "list": ["x", "pwd=secret123"]});
        let mut c = RedactionCounts::new();
        r.redact_json(&mut v, &mut c);
        assert_eq!(v["email"], "[REDACTED:email]");
        assert_eq!(v["list"][1], "pwd=[REDACTED:secret]");
    }
}
