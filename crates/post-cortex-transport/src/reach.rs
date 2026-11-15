// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The reach of a call (`docs/connect.md`, reach): a call that carries
//! `x-pcx-reach` may reach only those workspaces. [`Reached`] stands in front
//! of a memory service (`pcx.v1`, its blobs, `pcx.views.v1`), served to
//! clients and to the modules, whose calls carry the reach of the call they
//! serve: it reads the call as the operation table sees it (`classify`) and
//! refuses one that names a workspace outside the reach, a session of no
//! workspace in it, or every workspace but the lists of workspaces and
//! sessions, whose answers it keeps to the reach. The memory instance is the
//! data's owner: it does not trust whoever forwarded the call to have
//! checked. For the same reason it refuses here, on every way in, a call
//! that would change an archived workspace (`post_cortex_memory::archive`),
//! with a reach or without one.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::State;
use axum::middleware::Next;
use axum::response::IntoResponse;
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use http::StatusCode;
use http::{Request, Response};
use http_body::Body as _;
use pcx_module_api::metadata;
use post_cortex_memory::Memory;
use post_cortex_proto::pb::{ListSessionsResponse, ListWorkspacesResponse};
use prost::Message as _;
use tonic::Status;
use tonic::body::Body;
use tonic::server::NamedService;
use tower::{Service, ServiceExt};
use uuid::Uuid;

use crate::classify;

/// The most bytes of a call's first message read to know what it reaches.
const MAX_FIRST_MESSAGE: usize = 8 * 1024 * 1024;

/// A memory service `S` that refuses a call outside its reach.
#[derive(Clone)]
pub struct Reached<S> {
    inner: S,
    memory: Arc<Memory>,
}

impl<S> Reached<S> {
    /// `inner`, its calls checked against `memory`'s workspaces.
    pub fn new(inner: S, memory: Arc<Memory>) -> Self {
        Self { inner, memory }
    }
}

impl<S: NamedService> NamedService for Reached<S> {
    const NAME: &'static str = S::NAME;
}

impl<S> Service<Request<Body>> for Reached<S>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let inner = self.inner.clone();
        let memory = Arc::clone(&self.memory);
        Box::pin(async move {
            let (req, list) = match within(req, &memory).await {
                Ok(within) => within,
                Err(status) => return Ok(status.into_http()),
            };
            let answer = inner.oneshot(req).await?;
            match list {
                None => Ok(answer),
                Some(list) => Ok(kept(answer, &list, &memory)
                    .await
                    .unwrap_or_else(|status| status.into_http())),
            }
        })
    }
}

/// A list of every workspace or session whose answer is kept to a reach.
struct List {
    /// The RPC.
    path: String,
    /// The reach.
    reach: Vec<Uuid>,
}

/// `req` again, once it is within its reach (a call with none is), and the
/// list whose answer is kept to it, if the call is one.
async fn within(
    req: Request<Body>,
    memory: &Memory,
) -> Result<(Request<Body>, Option<List>), Status> {
    let reach = reach(req.headers())?;
    // A call without a reach is read only when it may change memory.
    if reach.is_none() && !classify::grpc_changes(req.uri().path()) {
        return Ok((req, None));
    }
    let for_person = person(req.headers())?;
    let (parts, body) = req.into_parts();
    let (read, rest) = first_message(body).await?;
    let path = parts.uri.path().to_string();
    let classified = message_of(&read).and_then(|message| {
        classify::grpc(&path, message)
            .map_err(|e| Status::permission_denied(format!("a call with a reach: {e}")))
    });
    let list = match (classified, reach) {
        // Without a reach, a call that does not read as its operation goes
        // on to its handler, which refuses it in its own words.
        (Err(_), None) => None,
        (Err(status), Some(_)) => return Err(status),
        (Ok(classified), reach) => {
            memory
                .open_to(&classified)
                .map_err(|e| Status::failed_precondition(e.to_string()))?;
            match reach {
                Some(reach) if classified.instance_wide() && listed(&path) => {
                    Some(List { path, reach })
                }
                Some(reach) => {
                    memory
                        .within_reach(&classified, &reach, for_person)
                        .await
                        .map_err(|e| Status::permission_denied(e.to_string()))?;
                    None
                }
                None => None,
            }
        }
    };
    let replay = futures::stream::once(async move { Ok(http_body::Frame::data(read)) })
        .chain(http_body_util::BodyStream::new(rest));
    Ok((
        Request::from_parts(parts, Body::new(http_body_util::StreamBody::new(replay))),
        list,
    ))
}

/// The lists kept to a reach.
const WORKSPACES: &str = "/pcx.v1.PostCortex/ListWorkspaces";
const SESSIONS: &str = "/pcx.v1.PostCortex/ListSessions";

fn listed(path: &str) -> bool {
    path == WORKSPACES || path == SESSIONS
}

/// The answer of `list` with only what its reach covers.
///
/// # Errors
/// `Internal` for an answer that does not read as the list's.
async fn kept(
    answer: Response<Body>,
    list: &List,
    memory: &Memory,
) -> Result<Response<Body>, Status> {
    let (parts, body) = answer.into_parts();
    let collected = http_body_util::BodyExt::collect(body)
        .await
        .map_err(|e| Status::internal(format!("the list's answer: {e}")))?;
    let trailers = collected.trailers().cloned();
    let read = collected.to_bytes();
    // A failure (trailers only) or an empty answer goes back as it came.
    if read.is_empty() {
        return Ok(rebuilt(parts, read, trailers));
    }
    let message = message_of(&read)?;
    let encoded = if list.path == WORKSPACES {
        let mut all = ListWorkspacesResponse::decode(message)
            .map_err(|e| Status::internal(format!("the list of workspaces: {e}")))?;
        all.workspaces
            .retain(|w| Uuid::parse_str(&w.workspace_id).is_ok_and(|id| list.reach.contains(&id)));
        all.encode_to_vec()
    } else {
        let mut all = ListSessionsResponse::decode(message)
            .map_err(|e| Status::internal(format!("the list of sessions: {e}")))?;
        let reached = memory.sessions_in_reach(&list.reach);
        all.sessions
            .retain(|s| Uuid::parse_str(&s.session_id).is_ok_and(|id| reached.contains(&id)));
        all.encode_to_vec()
    };
    let mut framed = BytesMut::with_capacity(5 + encoded.len());
    framed.extend_from_slice(&[0]);
    let len = u32::try_from(encoded.len())
        .map_err(|_| Status::internal("the kept list is too large for a gRPC message"))?;
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(&encoded);
    Ok(rebuilt(parts, framed.freeze(), trailers))
}

/// An answer of `parts` with `data`, then `trailers`.
fn rebuilt(
    mut parts: http::response::Parts,
    data: Bytes,
    trailers: Option<http::HeaderMap>,
) -> Response<Body> {
    parts.headers.remove(http::header::CONTENT_LENGTH);
    let mut frames = vec![Ok::<_, Infallible>(http_body::Frame::data(data))];
    if let Some(trailers) = trailers {
        frames.push(Ok(http_body::Frame::trailers(trailers)));
    }
    Response::from_parts(
        parts,
        Body::new(http_body_util::StreamBody::new(futures::stream::iter(
            frames,
        ))),
    )
}

/// The reach the call carries; `None`: every workspace.
///
/// # Errors
/// `InvalidArgument` for a reach that is not workspace ids.
fn reach(headers: &http::HeaderMap) -> Result<Option<Vec<Uuid>>, Status> {
    let Some(value) = headers.get(metadata::REACH) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| Status::invalid_argument("the call's reach is not text"))?;
    post_cortex_memory::reach::workspaces(&metadata::reach_of(value))
        .map(Some)
        .map_err(|e| Status::invalid_argument(e.to_string()))
}

/// Whether the call is for a person (`x-pcx-person`), whose own user card
/// is within any reach.
///
/// # Errors
/// `InvalidArgument` for a person that is no user id.
fn person(headers: &http::HeaderMap) -> Result<bool, Status> {
    crate::person::of_headers(headers)
        .map(|person| person.is_some())
        .map_err(|(_, message)| Status::invalid_argument(message))
}

/// The bytes of `body` up to the end of its first gRPC message (all of it
/// when shorter), and the rest of it.
async fn first_message(mut body: Body) -> Result<(Bytes, Body), Status> {
    let mut read = BytesMut::new();
    loop {
        if let Some(len) = frame_len(&read)
            && read.len() >= 5 + len
        {
            return Ok((read.freeze(), body));
        }
        if read.len() > MAX_FIRST_MESSAGE {
            return Err(Status::resource_exhausted(
                "the call's first message is too large to check its reach",
            ));
        }
        match std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
            None => return Ok((read.freeze(), body)),
            Some(Err(e)) => return Err(Status::invalid_argument(format!("the call's body: {e}"))),
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    read.extend_from_slice(&data);
                }
            }
        }
    }
}

/// The length of the gRPC message `read` begins with, once its header is in.
fn frame_len(read: &[u8]) -> Option<usize> {
    let header: [u8; 4] = read.get(1..5)?.try_into().ok()?;
    Some(u32::from_be_bytes(header) as usize)
}

/// The first message of a gRPC body that begins with it.
///
/// # Errors
/// `InvalidArgument` for a compressed or cut message.
fn message_of(read: &[u8]) -> Result<&[u8], Status> {
    match (read.first(), frame_len(read)) {
        (None, _) => Ok(&[]),
        (Some(0), Some(len)) if read.len() >= 5 + len => Ok(&read[5..5 + len]),
        (Some(0), _) => Err(Status::invalid_argument("the call's first message is cut")),
        (Some(_), _) => Err(Status::invalid_argument(
            "a call with a reach is not compressed",
        )),
    }
}

/// The most bytes of a REST request read to know what it reaches.
const MAX_REST_BODY: usize = 8 * 1024 * 1024;

/// axum middleware of the REST routes (`/api/*`): a request that carries a
/// reach is held to it as [`Reached`] holds a gRPC call; the lists of
/// workspaces and sessions answer only those within it. `/health` is no
/// operation.
pub async fn rest(
    State(memory): State<Arc<Memory>>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let reach = match reach(req.headers()) {
        Ok(reach) => reach,
        Err(status) => return refusal(StatusCode::BAD_REQUEST, status.message()),
    };
    // Without a reach only a request that may change memory is read.
    if req.uri().path() == "/health" || (reach.is_none() && req.method() == http::Method::GET) {
        return next.run(req).await;
    }
    let for_person = match person(req.headers()) {
        Ok(for_person) => for_person,
        Err(status) => return refusal(StatusCode::BAD_REQUEST, status.message()),
    };
    let (parts, body) = req.into_parts();
    let Ok(read) = axum::body::to_bytes(body, MAX_REST_BODY).await else {
        return refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the request is too large to check its reach",
        );
    };
    let again = |parts, read: Bytes| {
        next.run(axum::extract::Request::from_parts(
            parts,
            axum::body::Body::from(read),
        ))
    };
    let json = if read.is_empty() {
        Ok(serde_json::Value::Null)
    } else {
        serde_json::from_slice(&read)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("the body: {e}")))
    };
    let call = json.and_then(|json| {
        classify::rest(
            parts.method.as_str(),
            parts.uri.path(),
            parts.uri.query(),
            &json,
        )
        .map_err(|e| (StatusCode::FORBIDDEN, format!("a call with a reach: {e}")))
    });
    let (call, reach) = match (call, reach) {
        // Without a reach, a request that does not read as its operation
        // goes on to its handler, which refuses it in its own words.
        (Err(_), None) => return again(parts, read).await,
        (Err((status, message)), Some(_)) => return refusal(status, &message),
        (Ok(call), reach) => (call, reach),
    };
    if let Err(e) = memory.open_to(&call) {
        return refusal(StatusCode::CONFLICT, &e.to_string());
    }
    let Some(reach) = reach else {
        return again(parts, read).await;
    };
    let list =
        call.instance_wide() && matches!(call.operation.name, "workspace.list" | "session.list");
    if !list && let Err(e) = memory.within_reach(&call, &reach, for_person).await {
        return refusal(StatusCode::FORBIDDEN, &e.to_string());
    }
    let answer = again(parts, read).await;
    if !list || !answer.status().is_success() {
        return answer;
    }
    let allowed = if call.operation.name == "workspace.list" {
        reach
    } else {
        memory.sessions_in_reach(&reach)
    };
    let (parts, body) = answer.into_parts();
    let Ok(read) = axum::body::to_bytes(body, usize::MAX).await else {
        return refusal(StatusCode::INTERNAL_SERVER_ERROR, "the list's answer");
    };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_slice(&read) else {
        return refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the list's answer is no JSON list: not kept to the reach",
        );
    };
    let kept: Vec<serde_json::Value> = items
        .into_iter()
        .filter(|item| {
            item.get("id")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .is_some_and(|id| allowed.contains(&id))
        })
        .collect();
    let mut parts = parts;
    parts.headers.remove(http::header::CONTENT_LENGTH);
    axum::response::Response::from_parts(
        parts,
        axum::body::Body::from(serde_json::Value::Array(kept).to_string()),
    )
}

/// axum middleware of the browser transfer's uploads (`POST /v1/import`,
/// `/v1/export`, `/v1/blobs`, `/v1/blobs/link`): a request that carries a
/// reach is held to it as the RPC it maps to ([`classify::browser`]); an
/// import's archive and an image are not read.
pub async fn browser(
    State(memory): State<Arc<Memory>>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let reach = match reach(req.headers()) {
        Ok(reach) => reach,
        Err(status) => return refusal(StatusCode::BAD_REQUEST, status.message()),
    };
    let for_person = match person(req.headers()) {
        Ok(for_person) => for_person,
        Err(status) => return refusal(StatusCode::BAD_REQUEST, status.message()),
    };
    let path = req.uri().path().to_string();
    let (parts, body) = req.into_parts();
    // Only the export and the link have a JSON body that says what they
    // reach; the others say it in their query.
    let (read, body) = if path == "/v1/export" || path == "/v1/blobs/link" {
        let Ok(read) = axum::body::to_bytes(body, MAX_REST_BODY).await else {
            return refusal(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the request is too large to check its reach",
            );
        };
        (read.clone(), axum::body::Body::from(read))
    } else {
        (Bytes::new(), body)
    };
    let again = axum::extract::Request::from_parts(parts.clone(), body);
    let call = match classify::browser(parts.method.as_str(), &path, parts.uri.query(), &read) {
        Ok(call) => call,
        // Without a reach its handler refuses it in its own words.
        Err(_) if reach.is_none() => return next.run(again).await,
        Err(e) => return refusal(StatusCode::FORBIDDEN, &format!("a call with a reach: {e}")),
    };
    if let Err(e) = memory.open_to(&call) {
        return refusal(StatusCode::CONFLICT, &e.to_string());
    }
    if let Some(reach) = reach
        && let Err(e) = memory.within_reach(&call, &reach, for_person).await
    {
        return refusal(StatusCode::FORBIDDEN, &e.to_string());
    }
    next.run(again).await
}

/// A REST refusal with `message`.
fn refusal(status: StatusCode, message: &str) -> axum::response::Response {
    (status, format!("{message}\n")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reach_is_ids_or_none() {
        let mut headers = http::HeaderMap::new();
        assert_eq!(reach(&headers).unwrap(), None);
        headers.insert(metadata::REACH, metadata::NO_WORKSPACE.parse().unwrap());
        assert_eq!(reach(&headers).unwrap(), Some(Vec::new()));
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        headers.insert(metadata::REACH, format!("{a},{b}").parse().unwrap());
        assert_eq!(reach(&headers).unwrap(), Some(vec![a, b]));
        headers.insert(metadata::REACH, "not-an-id".parse().unwrap());
        assert_eq!(
            reach(&headers).unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
    }

    #[test]
    fn the_first_message_is_read_from_its_frame() {
        let mut framed = vec![0u8, 0, 0, 0, 3];
        framed.extend_from_slice(b"abcdef");
        assert_eq!(message_of(&framed).unwrap(), b"abc");
        assert!(message_of(&[0, 0, 0, 0, 9, 1]).is_err());
        assert!(message_of(&[1, 0, 0, 0, 0]).is_err());
        assert_eq!(message_of(&[]).unwrap(), b"");
    }
}
