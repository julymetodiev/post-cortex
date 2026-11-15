// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief's `pcx.chief.v1.Chief` service (W23): each RPC's message and
//! the call's scope in (who asks, through which module), the chief
//! service's call, its message out. No logic of their own.

use pcx_documents::Item;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::chief::chief_server::Chief as ChiefService;
use post_cortex_proto::chief::{
    ActionList, ActionRecord, AskRequest, AskResponse, GetActionRequest, GetRightsRequest,
    ListActionsRequest, ListTalksRequest, Rights, TalkList, TalkRecord,
};
use tonic::{Request, Response, Status};

use crate::domain::{Action, Talk};
use crate::service::{Asker, Conversation};
use crate::wiring::{self, Shared, Wired};

/// The chief's RPCs over the running chief.
pub struct Handlers {
    /// The running chief.
    pub shared: Shared,
}

impl Handlers {
    /// The chief's ports for a call in `scope`.
    async fn wired(&self, scope: &Scope) -> Result<Wired, Status> {
        Ok(Wired::new(wiring::running(&self.shared)?, scope).await?)
    }
}

fn talk(item: Item<Talk>) -> TalkRecord {
    TalkRecord {
        id: item.document.id,
        talk: Some(item.data),
        question: item.document.text,
        updated_at: item.document.updated_at,
    }
}

fn action(item: Item<Action>) -> ActionRecord {
    ActionRecord {
        id: item.document.id,
        action: Some(item.data),
        updated_at: item.document.updated_at,
    }
}

#[tonic::async_trait]
impl ChiefService for Handlers {
    async fn ask(&self, request: Request<AskRequest>) -> Result<Response<AskResponse>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let author = scope.author.clone().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                "Ask needs the caller's author: who asks",
            )
        })?;
        let channel = scope.caller_module.clone().unwrap_or_default();
        let wired = self.wired(&scope).await?;
        // A task of its own: a turn takes minutes and outlives a connection
        // that drops, its answer kept in its conversation.
        let (answer, answered) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let asker = Asker {
                author: &author,
                channel: &channel,
            };
            let conversation = match request.conversation_id.as_str() {
                "" => Conversation::New,
                id => Conversation::Talk(id),
            };
            let ports = wired.ports();
            let asked = wired
                .chief(&ports)
                .ask(asker, &request.question, conversation, None)
                .await;
            if answer.send(asked).is_err() {
                tracing::info!("whoever asked the chief left; its answer is in the conversation");
            }
        });
        let asked = answered.await.map_err(|e| {
            Status::internal(format!("the chief's turn ended without an answer: {e}"))
        })??;
        Ok(Response::new(AskResponse {
            conversation_id: asked.conversation,
            end: Some(asked.end.end_wire()),
        }))
    }

    async fn get_rights(
        &self,
        request: Request<GetRightsRequest>,
    ) -> Result<Response<Rights>, Status> {
        let scope = Scope::of(&request)?;
        let wired = self.wired(&scope).await?;
        let ports = wired.ports();
        Ok(Response::new(wired.chief(&ports).rights().await?))
    }

    async fn list_talks(
        &self,
        request: Request<ListTalksRequest>,
    ) -> Result<Response<TalkList>, Status> {
        let scope = Scope::of(&request)?;
        let limit = request.get_ref().limit;
        let wired = self.wired(&scope).await?;
        let ports = wired.ports();
        let talks = wired.chief(&ports).talks(limit).await?;
        Ok(Response::new(TalkList {
            talks: talks.into_iter().map(talk).collect(),
        }))
    }

    async fn list_actions(
        &self,
        request: Request<ListActionsRequest>,
    ) -> Result<Response<ActionList>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let wired = self.wired(&scope).await?;
        let ports = wired.ports();
        let actions = wired
            .chief(&ports)
            .actions(
                Some(request.conversation_id.as_str()).filter(|c| !c.is_empty()),
                request.limit,
            )
            .await?;
        Ok(Response::new(ActionList {
            actions: actions.into_iter().map(action).collect(),
        }))
    }

    async fn get_action(
        &self,
        request: Request<GetActionRequest>,
    ) -> Result<Response<ActionRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let wired = self.wired(&scope).await?;
        let ports = wired.ports();
        let found = wired.chief(&ports).action(&request.id).await?;
        Ok(Response::new(action(found)))
    }
}
