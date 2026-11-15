// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the module hears: the events it publishes and its Gateway's
//! connections.

use super::*;

fn message(id: &str, channel: &str, author: &str, bot: bool) -> Value {
    json!({
        "id": id, "channel_id": channel, "guild_id": GUILD, "content": "how?",
        "author": { "id": author, "username": "ana", "global_name": null, "bot": bot },
        "member": { "nick": null, "roles": [] },
        "message_reference": null,
    })
}

#[tokio::test]
async fn what_people_write_where_it_hears_is_published_and_nothing_else() {
    let world = World::new().await;
    let service = world.service();
    // A new post of the forum: its thread, then its first message.
    let created = json!({
        "id": "300", "type": 11, "guild_id": GUILD, "parent_id": "200",
        "name": "How do I pair a runner?", "applied_tags": ["71"], "newly_created": true,
    });
    service.heard("THREAD_CREATE", created).await.unwrap();
    service
        .heard("MESSAGE_CREATE", message("300", "300", "7", false))
        .await
        .unwrap();
    service
        .heard("MESSAGE_CREATE", message("301", "300", "7", false))
        .await
        .unwrap();
    service
        .heard("MESSAGE_CREATE", message("302", "201", "7", false))
        .await
        .unwrap();
    // A bot's, the bot's own, a channel the settings do not name, a thread
    // Discord does not know, an event it does not read.
    service
        .heard("MESSAGE_CREATE", message("303", "201", "8", true))
        .await
        .unwrap();
    service
        .heard("MESSAGE_CREATE", message("304", "300", "9", false))
        .await
        .unwrap();
    service
        .heard("MESSAGE_CREATE", message("305", "999", "7", false))
        .await
        .unwrap();
    service.heard("TYPING_START", json!({})).await.unwrap();

    let heard = world.bus.0.lock().clone();
    assert_eq!(heard.len(), 3, "{heard:?}");
    let Heard::Post(post) = &heard[0] else {
        panic!("the post was not heard first: {heard:?}");
    };
    assert_eq!(
        (
            post.channel.as_str(),
            post.title.as_str(),
            post.tags.as_slice()
        ),
        (
            "help",
            "How do I pair a runner?",
            &["answered".to_string()][..]
        )
    );
    assert!(matches!(&heard[1], Heard::Message(m) if m.thread_id == "300"));
    assert!(matches!(&heard[2], Heard::Message(m) if m.channel == "general"));
    // The thread came with the Gateway's event: Discord was asked only of
    // the channel no one named, once.
    assert_eq!(*world.discord.asked.lock(), ["999"]);

    let malformed = service.heard("MESSAGE_CREATE", json!({ "id": 5 })).await;
    assert!(malformed.is_err());
}

/// A Gateway that plays one connection's frames, then closes it.
struct Scripted {
    frames: Mutex<VecDeque<Value>>,
    sent: Arc<Mutex<Vec<Sent>>>,
    urls: Mutex<Vec<String>>,
}

struct Played {
    frames: VecDeque<Value>,
    sent: Arc<Mutex<Vec<Sent>>>,
}

#[async_trait]
impl Gateway for Scripted {
    async fn connect(&self, url: &str) -> Result<Box<dyn Link>, ModuleError> {
        self.urls.lock().push(url.to_string());
        Ok(Box::new(Played {
            frames: std::mem::take(&mut *self.frames.lock()),
            sent: Arc::clone(&self.sent),
        }))
    }
}

#[async_trait]
impl Link for Played {
    async fn next(&mut self) -> Result<Option<Frame>, ModuleError> {
        Ok(self
            .frames
            .pop_front()
            .map(|frame| serde_json::from_value(frame).unwrap()))
    }

    async fn send(&mut self, sent: &Sent) -> Result<(), ModuleError> {
        self.sent.lock().push(sent.clone());
        Ok(())
    }
}

#[tokio::test]
async fn a_connection_identifies_hears_its_events_and_the_next_one_resumes() {
    let world = World::new().await;
    let gateway = Scripted {
        frames: Mutex::new(VecDeque::from([
            json!({ "op": 10, "d": { "heartbeat_interval": 45000 } }),
            json!({ "op": 0, "s": 1, "t": "READY", "d": {
                "session_id": "sess-1", "resume_gateway_url": "wss://resume.example",
            } }),
            json!({ "op": 0, "s": 2, "t": "MESSAGE_CREATE", "d": message("302", "201", "7", false) }),
            // One that cannot be read does not end the connection.
            json!({ "op": 0, "s": 3, "t": "MESSAGE_CREATE", "d": { "id": 5 } }),
            json!({ "op": 0, "s": 4, "t": "GUILD_MEMBER_ADD", "d": {
                "guild_id": GUILD, "user": { "id": "175928847299117063", "username": "ana" },
            } }),
        ])),
        sent: Arc::default(),
        urls: Mutex::default(),
    };
    let (mut session, stop) = (Session::default(), CancellationToken::new());
    let service = world.service();
    service.listen(&gateway, &mut session, &stop).await.unwrap();
    assert_eq!(gateway.sent.lock()[0].op, 2, "an identify");
    assert_eq!(world.bus.0.lock().len(), 2, "a message and a member");

    // The Gateway closed it: the next connection resumes where it was.
    *gateway.frames.lock() =
        VecDeque::from([json!({ "op": 10, "d": { "heartbeat_interval": 45000 } })]);
    service.listen(&gateway, &mut session, &stop).await.unwrap();
    assert_eq!(
        *gateway.urls.lock(),
        ["wss://gateway.example", "wss://resume.example"]
    );
    let resumed = gateway.sent.lock()[1].clone();
    assert_eq!(resumed.op, 6);
    assert_eq!(resumed.d["seq"], 4);

    // Stopped, it listens no more.
    stop.cancel();
    service.listen(&gateway, &mut session, &stop).await.unwrap();
    assert_eq!(gateway.sent.lock().len(), 2);
}
