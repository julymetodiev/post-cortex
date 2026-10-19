// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Voyage AI rerank API (`POST /v1/rerank`). One request per search with the
//! whole candidate pool. On LoCoMo `rerank-3-lite` over a pool of 300 gives
//! all@50 0.938 with ±1 neighbours, against 0.878 for the local MiniLM-L6.
//! Candidate text is sent to Voyage, so this is opt-in.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::Reranker;

const URL: &str = "https://api.voyageai.com/v1/rerank";

/// Characters per Voyage token, measured on LoCoMo and LongMemEval records;
/// used to cut documents without a tokenizer.
const CHARS_PER_TOKEN: f32 = 3.83;

/// Requests that get a 429 or 5xx are retried this many times.
const RETRIES: u32 = 2;

/// Reranks with a Voyage model such as `rerank-3-lite` or `rerank-3`.
pub struct VoyageReranker {
    client: reqwest::Client,
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
    /// `max_doc_tokens` cuts each document to about that many tokens before
    /// sending: 256 cost 31% fewer tokens with no loss on LongMemEval.
    pub fn new(
        api_key: String,
        model: String,
        max_doc_tokens: usize,
        timeout: Duration,
    ) -> Result<Self> {
        let client = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            client,
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
    async fn score(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
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
                .post(URL)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await?;
            let status = response.status();
            if status.is_success() {
                break response;
            }
            if attempt < RETRIES && (status.as_u16() == 429 || status.is_server_error()) {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
                continue;
            }
            let text = response.text().await.unwrap_or_default();
            bail!(
                "Voyage rerank returned {status}: {}",
                text.chars().take(300).collect::<String>()
            );
        };

        let parsed: Response = response.json().await?;
        let mut scores = vec![None; documents.len()];
        for s in parsed.data {
            let slot = scores.get_mut(s.index).ok_or_else(|| {
                anyhow!(
                    "Voyage returned index {} for {} documents",
                    s.index,
                    documents.len()
                )
            })?;
            *slot = Some(s.relevance_score);
        }
        scores
            .into_iter()
            .enumerate()
            .map(|(i, s)| s.ok_or_else(|| anyhow!("Voyage returned no score for document {i}")))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reranker(max_doc_tokens: usize) -> VoyageReranker {
        VoyageReranker::new(
            "test".into(),
            "rerank-3-lite".into(),
            max_doc_tokens,
            Duration::from_secs(1),
        )
        .unwrap()
    }

    #[test]
    fn cuts_documents_on_char_boundaries() {
        let r = reranker(2); // 7 characters
        assert_eq!(r.cut("short"), "short");
        assert_eq!(r.cut("Здравей, свят"), "Здравей");
    }

    #[tokio::test]
    #[ignore = "calls the Voyage API; needs VOYAGE_API_KEY"]
    async fn ranks_the_answer_first() {
        let key = std::env::var("VOYAGE_API_KEY").expect("VOYAGE_API_KEY");
        let r =
            VoyageReranker::new(key, "rerank-3-lite".into(), 256, Duration::from_secs(30)).unwrap();
        let docs = vec![
            "Melanie: I went camping with the kids last weekend.".to_string(),
            "Caroline: I researched adoption agencies yesterday.".to_string(),
            "Melanie: The weather has been great lately.".to_string(),
        ];
        let scores = r.score("What did Caroline research?", &docs).await.unwrap();
        assert_eq!(scores.len(), 3);
        assert!(scores[1] > scores[0] && scores[1] > scores[2], "{scores:?}");
        assert!(scores.iter().all(|s| (0.0..=1.0).contains(s)));
    }
}
