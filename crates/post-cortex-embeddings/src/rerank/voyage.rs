// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Voyage AI rerank API (`POST /v1/rerank`). One request per search with the
//! whole candidate pool. On LoCoMo `rerank-3-lite` over a pool of 300 gives
//! all@50 0.938 with ±1 neighbours, against 0.878 for the local MiniLM-L6.
//! Candidate text is sent to Voyage, so this is opt-in.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use post_cortex_ports::error::Result;
use post_cortex_ports::{PortError, Reranker};

/// Characters per Voyage token, measured on LoCoMo and LongMemEval records;
/// used to cut documents without a tokenizer.
const CHARS_PER_TOKEN: f32 = 3.83;

/// Requests that get a 429 or 5xx are retried this many times.
const RETRIES: u32 = 2;

/// Reranks with a Voyage model such as `rerank-3-lite` or `rerank-3`.
pub struct VoyageReranker {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    max_chars: usize,
}

#[derive(Serialize)]
struct Request<'a> {
    query: &'a str,
    documents: Vec<&'a str>,
    model: &'a str,
    truncation: bool,
}

#[derive(Deserialize)]
struct Response {
    data: Vec<Scored>,
}

#[derive(Deserialize)]
struct Scored {
    index: usize,
    relevance_score: f32,
}

impl VoyageReranker {
    /// Reranks through the rerank API at `endpoint`
    /// ([`crate::rerank::VOYAGE_ENDPOINT`] for Voyage itself). `max_doc_tokens` cuts
    /// each document to about that many tokens before sending: 256 cost 31%
    /// fewer tokens with no loss on LongMemEval.
    ///
    /// # Errors
    /// The system refuses the HTTP client.
    pub fn new(
        endpoint: String,
        api_key: String,
        model: String,
        max_doc_tokens: usize,
        timeout: Duration,
    ) -> std::result::Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            client,
            endpoint,
            api_key,
            model,
            max_chars: (max_doc_tokens as f32 * CHARS_PER_TOKEN) as usize,
        })
    }

    fn cut<'a>(&self, doc: &'a str) -> &'a str {
        match doc.char_indices().nth(self.max_chars) {
            Some((end, _)) => &doc[..end],
            None => doc,
        }
    }
}

#[async_trait]
impl Reranker for VoyageReranker {
    /// One relevance per document, in input order. A transport failure, an
    /// error status after the retries, or an answer that does not score
    /// every document once is `Unavailable`.
    async fn score(&self, query: &str, documents: &[Arc<str>]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let body = Request {
            query,
            documents: documents.iter().map(|d| self.cut(d)).collect(),
            model: &self.model,
            truncation: true,
        };
        let mut attempt = 0;
        let response = loop {
            let response = self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(unavailable)?;
            let status = response.status();
            if status.is_success() {
                break response;
            }
            if attempt < RETRIES && (status.as_u16() == 429 || status.is_server_error()) {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
                continue;
            }
            let text = match response.text().await {
                Ok(text) => text.chars().take(300).collect::<String>(),
                Err(e) => format!("(body unreadable: {e})"),
            };
            return Err(unavailable(format!(
                "Voyage rerank returned {status}: {text}"
            )));
        };

        let parsed: Response = response.json().await.map_err(unavailable)?;
        let mut scores = vec![None; documents.len()];
        for s in parsed.data {
            let slot = scores.get_mut(s.index).ok_or_else(|| {
                unavailable(format!(
                    "Voyage returned index {} for {} documents",
                    s.index,
                    documents.len()
                ))
            })?;
            *slot = Some(s.relevance_score);
        }
        scores
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                s.ok_or_else(|| unavailable(format!("Voyage returned no score for document {i}")))
            })
            .collect()
    }
}

/// A failure of the rerank service.
fn unavailable(e: impl std::fmt::Display) -> PortError {
    PortError::Unavailable(format!("Voyage rerank: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::{Json, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use serde_json::{Value, json};

    const KEY: &str = "test-key";

    /// The rerank API's address on a local port that `router` answers.
    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}/v1/rerank")
    }

    fn reranker(endpoint: String, key: &str, max_doc_tokens: usize) -> VoyageReranker {
        VoyageReranker::new(
            endpoint,
            key.into(),
            "rerank-3-lite".into(),
            max_doc_tokens,
            Duration::from_secs(5),
        )
        .unwrap()
    }

    /// A stand-in for the rerank API: checks the key, scores each document
    /// by its text alone and answers best first, as Voyage does.
    async fn rerank(headers: HeaderMap, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
        let bearer = format!("Bearer {KEY}");
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(bearer.as_str()) {
            return (StatusCode::UNAUTHORIZED, Json(json!({"detail": "bad key"})));
        }
        let mut data: Vec<(usize, f64)> = body["documents"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, d)| {
                let sum: u32 = d.as_str().unwrap().bytes().map(u32::from).sum();
                (index, f64::from(sum % 97) / 97.0)
            })
            .collect();
        data.sort_by(|a, b| b.1.total_cmp(&a.1));
        let data: Vec<Value> = data
            .into_iter()
            .map(|(index, score)| json!({"index": index, "relevance_score": score}))
            .collect();
        (StatusCode::OK, Json(json!({ "data": data })))
    }

    /// The adapter over the stand-in, for the port's conformance suite.
    pub async fn stand_in() -> (Arc<dyn Reranker>, ()) {
        let endpoint = serve(Router::new().route("/v1/rerank", post(rerank))).await;
        (Arc::new(reranker(endpoint, KEY, 256)), ())
    }

    mod conformance {
        post_cortex_ports::rerank_conformance_suite!(crate::rerank::voyage::tests::stand_in());
    }

    #[test]
    fn cuts_documents_on_char_boundaries() {
        let r = reranker(String::new(), KEY, 2); // 7 characters
        assert_eq!(r.cut("short"), "short");
        assert_eq!(r.cut("Здравей, свят"), "Здравей");
    }

    /// A refused key and a failing service are `Unavailable`; 5xx answers
    /// are retried, a 401 is not.
    #[tokio::test]
    async fn failures_are_unavailable_and_server_errors_retried() {
        let endpoint = serve(Router::new().route("/v1/rerank", post(rerank))).await;
        let refused = reranker(endpoint, "wrong", 256)
            .score("q", &[Arc::from("d")])
            .await
            .unwrap_err();
        assert!(
            matches!(refused, PortError::Unavailable(ref m) if m.contains("401")),
            "{refused}"
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let failing = Router::new()
            .route(
                "/v1/rerank",
                post(|State(calls): State<Arc<AtomicUsize>>| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    StatusCode::BAD_GATEWAY
                }),
            )
            .with_state(Arc::clone(&calls));
        let down = reranker(serve(failing).await, KEY, 256)
            .score("q", &[Arc::from("d")])
            .await
            .unwrap_err();
        assert!(matches!(down, PortError::Unavailable(_)), "{down}");
        assert_eq!(calls.load(Ordering::SeqCst), 1 + RETRIES as usize);
    }

    #[tokio::test]
    #[ignore = "calls the Voyage API; needs VOYAGE_API_KEY"]
    async fn ranks_the_answer_first() {
        let key = std::env::var("VOYAGE_API_KEY").expect("VOYAGE_API_KEY");
        let r = VoyageReranker::new(
            crate::rerank::VOYAGE_ENDPOINT.into(),
            key,
            "rerank-3-lite".into(),
            256,
            Duration::from_secs(30),
        )
        .unwrap();
        let docs: Vec<Arc<str>> = vec![
            "Melanie: I went camping with the kids last weekend.".into(),
            "Caroline: I researched adoption agencies yesterday.".into(),
            "Melanie: The weather has been great lately.".into(),
        ];
        let scores = r.score("What did Caroline research?", &docs).await.unwrap();
        assert!(scores[1] > scores[0] && scores[1] > scores[2], "{scores:?}");
    }
}
