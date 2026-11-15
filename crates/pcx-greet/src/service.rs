// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `Greeter` service: greets with the configured greeting and keeps
//! who it greeted, newest last.

use std::sync::Arc;

use tokio::sync::RwLock;
use tonic::{Request, Response, Status};

use crate::pb::greeter_server::Greeter;
use crate::pb::{
    ForgetRequest, Forgotten, GreetRequest, Greeting, Greetings, ListGreetingsRequest,
};

/// What the module knows: its greeting, and the greetings it gave.
#[derive(Default)]
pub struct State {
    /// The configured greeting; empty before the module starts.
    pub greeting: RwLock<String>,
    /// What is said after a greeting to someone; empty: nothing.
    pub signature: RwLock<String>,
    /// The greetings given to someone, oldest first.
    greeted: RwLock<Vec<Greeting>>,
}

/// The service over the module's state.
pub struct Service(pub Arc<State>);

#[tonic::async_trait]
impl Greeter for Service {
    async fn greet(&self, request: Request<GreetRequest>) -> Result<Response<Greeting>, Status> {
        let name = request.into_inner().name;
        let greeting = self.0.greeting.read().await.clone();
        if name.is_empty() {
            return Ok(Response::new(Greeting {
                name,
                text: greeting,
            }));
        }
        let signature = self.0.signature.read().await.clone();
        let text = if signature.is_empty() {
            format!("{greeting}, {name}!")
        } else {
            format!("{greeting}, {name}! {signature}")
        };
        let given = Greeting { text, name };
        self.0.greeted.write().await.push(given.clone());
        Ok(Response::new(given))
    }

    async fn list_greetings(
        &self,
        _: Request<ListGreetingsRequest>,
    ) -> Result<Response<Greetings>, Status> {
        let greetings: Vec<Greeting> = self.0.greeted.read().await.iter().rev().cloned().collect();
        Ok(Response::new(Greetings {
            count: greetings.len() as u64,
            greetings,
        }))
    }

    async fn forget(&self, _: Request<ForgetRequest>) -> Result<Response<Forgotten>, Status> {
        let mut greeted = self.0.greeted.write().await;
        let forgotten = greeted.len() as u64;
        greeted.clear();
        Ok(Response::new(Forgotten { forgotten }))
    }
}
