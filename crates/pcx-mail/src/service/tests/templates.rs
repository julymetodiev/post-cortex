// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Templates kept, checked, previewed and sent from.

use post_cortex_proto::mail::{Input, PreviewTemplateRequest, SaveTemplateRequest, Template};
use serde_json::json;

use super::*;

fn struct_of(value: serde_json::Value) -> pbjson_types::Struct {
    serde_json::from_value(value).unwrap()
}

fn reminder() -> Template {
    Template {
        name: "reminder".into(),
        inputs: vec![Input {
            name: "task".into(),
            r#type: "string".into(),
            example: Some(serde_json::from_value(json!("Paint the lighthouse")).unwrap()),
            description: String::new(),
        }],
        subject: "Reminder: {{ task }}".into(),
        text: "Do not forget: {{ task }}.".into(),
        ..Template::default()
    }
}

fn save(template: Template) -> SaveTemplateRequest {
    SaveTemplateRequest {
        template: Some(template),
        ..SaveTemplateRequest::default()
    }
}

#[tokio::test]
async fn a_template_is_saved_found_previewed_and_sent_from() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-10-31T09:30:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&[], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    let saved = service.save_template(save(reminder())).await.unwrap();
    assert_eq!(service.get_template("reminder").await.unwrap(), saved);
    assert_eq!(
        service.list_templates().await.unwrap().templates,
        vec![saved.clone()]
    );

    let preview = service
        .preview_template(PreviewTemplateRequest {
            name: "reminder".into(),
            ..PreviewTemplateRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(preview.subject, "Reminder: Paint the lighthouse");

    let record = service
        .send(
            SendRequest {
                to: vec!["ana@example.com".into()],
                template: "reminder".into(),
                values: Some(struct_of(json!({"task": "Close the gate"}))),
                ..SendRequest::default()
            },
            None,
        )
        .await
        .unwrap();
    let sent = transport.sent.lock().clone();
    assert_eq!(
        (
            sent[0].subject.as_str(),
            sent[0].text.as_str(),
            sent[0].html.as_deref()
        ),
        (
            "Reminder: Close the gate",
            "Do not forget: Close the gate.",
            None
        )
    );
    let entry = record.sent.unwrap();
    assert_eq!(entry.template, "reminder");
    let by_template = service
        .list_log(ListLogRequest {
            template: "reminder".into(),
            ..ListLogRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(by_template.entries.len(), 1);

    // A change names the version read; a second template takes no name
    // that is taken.
    let mut changed = reminder();
    changed.subject = "Soon: {{ task }}".into();
    let again = service
        .save_template(SaveTemplateRequest {
            id: saved.id.clone(),
            version: saved.version,
            template: Some(changed),
        })
        .await
        .unwrap();
    assert_eq!(again.version, saved.version + 1);
    let twin = service.save_template(save(reminder())).await.unwrap_err();
    assert_eq!(twin.kind, ModuleErrorKind::Conflict);
}

#[tokio::test]
async fn a_send_from_a_template_is_held_to_its_inputs() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-10-31T09:30:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&[], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    service.save_template(save(reminder())).await.unwrap();
    let send = |values: serde_json::Value, subject: &str| SendRequest {
        to: vec!["ana@example.com".into()],
        template: "reminder".into(),
        subject: subject.into(),
        values: Some(struct_of(values)),
        ..SendRequest::default()
    };
    let missing = service.send(send(json!({}), ""), None).await.unwrap_err();
    assert_eq!(
        (missing.kind, missing.field.as_deref()),
        (ModuleErrorKind::InvalidArgument, Some("values.task"))
    );
    let own = service
        .send(send(json!({"task": "x"}), "My own"), None)
        .await
        .unwrap_err();
    assert_eq!(own.field.as_deref(), Some("template"));
    // A value with a line break would make the subject two headers.
    let broken_line = service
        .send(send(json!({"task": "x\nBcc: all@example.com"}), ""), None)
        .await
        .unwrap_err();
    assert_eq!(broken_line.field.as_deref(), Some("subject"));
    let none = service
        .send(
            SendRequest {
                template: "nothing".into(),
                ..send(json!({}), "")
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(none.kind, ModuleErrorKind::NotFound);
    assert!(transport.sent.lock().is_empty());
    let loose = service
        .send(
            SendRequest {
                template: String::new(),
                ..send(json!({"task": "x"}), "A subject")
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(loose.field.as_deref(), Some("values"));
}
