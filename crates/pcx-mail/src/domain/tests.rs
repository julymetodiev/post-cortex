// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::mail::{Postmark, Resend, Smtp};

use super::*;

fn config() -> MailConfig {
    MailConfig {
        workspace_id: "ws-1".into(),
        from: Some(Mailbox {
            address: "reports@example.com".into(),
            name: "Reports".into(),
        }),
        transport: Transport::Smtp.into(),
        smtp: Some(Smtp {
            host: "smtp.example.com".into(),
            user: "reports".into(),
            password: "secret".into(),
            ..Smtp::default()
        }),
        ..MailConfig::default()
    }
}

fn message() -> SendRequest {
    SendRequest {
        to: vec!["ana@example.com".into()],
        subject: "The weekly report".into(),
        text: "All is well.".into(),
        ..SendRequest::default()
    }
}

fn field(result: Result<Settings, Broken>) -> String {
    result.unwrap_err().field
}

#[test]
fn smtp_settings_read_with_their_defaults() {
    let settings = settings(config()).unwrap();
    assert_eq!(
        settings.route,
        Route::Smtp {
            host: "smtp.example.com".into(),
            port: 587,
            secured: Secured::StartTls,
            credentials: Some(("reports".into(), "secret".into())),
        }
    );
    assert_eq!(settings.daily_limit, 200);
    assert_eq!(settings.reply_to, None);

    let mut tls = config();
    tls.smtp.as_mut().unwrap().security = Security::Tls.into();
    let Route::Smtp { port, secured, .. } = settings_of(tls) else {
        panic!("not smtp")
    };
    assert_eq!((port, secured), (465, Secured::Tls));
}

fn settings_of(config: MailConfig) -> Route {
    settings(config).unwrap().route
}

#[test]
fn the_providers_read_with_their_urls_and_stream() {
    let mut resend = config();
    resend.transport = Transport::Resend.into();
    resend.resend = Some(Resend {
        api_key: "re_1".into(),
        api_url: "http://127.0.0.1:9/".into(),
    });
    assert_eq!(
        settings_of(resend),
        Route::Resend {
            url: "http://127.0.0.1:9".into(),
            key: "re_1".into()
        }
    );
    let mut postmark = config();
    postmark.transport = Transport::Postmark.into();
    postmark.postmark = Some(Postmark {
        server_token: "pm".into(),
        ..Postmark::default()
    });
    assert_eq!(
        settings_of(postmark),
        Route::Postmark {
            url: "https://api.postmarkapp.com".into(),
            token: "pm".into(),
            stream: "outbound".into()
        }
    );
}

#[test]
fn settings_that_break_a_rule_name_their_field() {
    let mut c = config();
    c.workspace_id = " ".into();
    assert_eq!(field(settings(c)), "workspace_id");

    let mut c = config();
    c.from.as_mut().unwrap().address = "reports".into();
    assert_eq!(field(settings(c)), "from.address");

    let mut c = config();
    c.from.as_mut().unwrap().name = "Reports\r\nBcc: all@example.com".into();
    assert_eq!(field(settings(c)), "from.name");

    let mut c = config();
    c.transport = Transport::Unspecified.into();
    assert_eq!(field(settings(c)), "transport");

    let mut c = config();
    c.transport = Transport::Resend.into();
    assert_eq!(field(settings(c)), "resend");

    let mut c = config();
    c.smtp.as_mut().unwrap().security = Security::None.into();
    assert_eq!(field(settings(c)), "smtp.security");
    let mut c = config();
    let smtp = c.smtp.as_mut().unwrap();
    smtp.security = Security::None.into();
    smtp.host = "127.0.0.1".into();
    assert_eq!(
        settings_of(c.clone()),
        Route::Smtp {
            host: "127.0.0.1".into(),
            port: 25,
            secured: Secured::Plain,
            credentials: Some(("reports".into(), "secret".into())),
        }
    );

    let mut c = config();
    c.smtp.as_mut().unwrap().password = String::new();
    assert_eq!(field(settings(c)), "smtp.password");

    let mut c = config();
    c.smtp.as_mut().unwrap().port = 70_000;
    assert_eq!(field(settings(c)), "smtp.port");

    let mut c = config();
    c.allow = vec!["@example".into()];
    assert_eq!(field(settings(c)), "allow");

    let mut c = config();
    c.daily_limit = 10_001;
    assert_eq!(field(settings(c)), "daily_limit");
}

#[test]
fn allow_matches_addresses_and_domains_in_any_case() {
    let mut c = config();
    c.allow = vec!["Ana@Example.com".into(), "@team.example.org".into()];
    let allow = settings(c).unwrap().allow;
    assert!(allowed(&allow, "ana@example.com"));
    assert!(allowed(&allow, "BOB@team.example.org"));
    assert!(!allowed(&allow, "bob@example.com"));
    assert!(!allowed(&allow, "bob@sub.team.example.org"));
    assert!(allowed(&[], "anyone@anywhere.example"));
}

#[test]
fn a_message_is_checked_before_it_is_sent() {
    assert_eq!(check(&message()), Ok(()));
    let rule = |m: SendRequest| check(&m).unwrap_err().field;

    let mut m = message();
    m.to.clear();
    m.cc = vec!["ana@example.com".into()];
    assert_eq!(rule(m), "to");

    let mut m = message();
    m.bcc = vec!["not an address".into()];
    assert_eq!(rule(m), "bcc");

    let mut m = message();
    m.cc = (0..50).map(|i| format!("p{i}@example.com")).collect();
    assert_eq!(rule(m), "to");

    let mut m = message();
    m.subject = "Report\nBcc: all@example.com".into();
    assert_eq!(rule(m), "subject");

    let mut m = message();
    m.subject = "x".repeat(999);
    assert_eq!(rule(m), "subject");

    let mut m = message();
    m.text = " ".into();
    m.html = "<p>hi</p>".into();
    assert_eq!(rule(m), "text");

    let mut m = message();
    m.html = "x".repeat((1 << 20) + 1);
    assert_eq!(rule(m), "html");
}

#[test]
fn an_address_has_a_local_part_and_a_domain() {
    for good in ["a@b.co", "first.last+tag@mail.example.com"] {
        assert_eq!(address("to", good), Ok(()), "{good}");
    }
    for bad in [
        "", "a", "@b.co", "a@b", "a b@c.co", "<a@b.co>", "a@-b.co", "a,b@c.co",
    ] {
        assert!(address("to", bad).is_err(), "{bad}");
    }
}

#[test]
fn a_mailbox_quotes_its_name() {
    let plain = Mailbox {
        address: "a@b.co".into(),
        name: String::new(),
    };
    assert_eq!(mailbox_text(&plain), "a@b.co");
    let named = Mailbox {
        address: "a@b.co".into(),
        name: "Ops, \"night\" team".into(),
    };
    assert_eq!(mailbox_text(&named), "\"Ops, \\\"night\\\" team\" <a@b.co>");
}
