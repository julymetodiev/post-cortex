// // Integration tests for gRPC service — validates UpdateContext → QueryContext round-trip
// //
// // These tests verify that data stored via gRPC UpdateContext is correctly
// // retrievable via QueryContext, with proper interaction_type mapping and
// // content preservation.

// use post_cortex_memory::{ConversationMemorySystem, SystemConfig};
// use post_cortex::daemon::grpc_service::pb::post_cortex_client::PostCortexClient;
// use post_cortex::daemon::grpc_service::pb::*;
// use post_cortex::daemon::grpc_service::PcxGrpcService;
// use post_cortex::storage::traits::StorageBackendType;
// use std::sync::Arc;
// use tempfile::TempDir;
// use tokio::net::TcpListener;
// use tonic::transport::Channel;

// use serial_test::serial;

// /// Spin up an in-process gRPC server on a random port with SurrealDB kv-mem backend.
// /// This tests the SAME storage path as production (SurrealDB), not RocksDB.
// async fn setup_grpc() -> (PostCortexClient<Channel>, TempDir) {
//     let temp_dir = tempfile::tempdir().unwrap();

//     let mut config = SystemConfig::default();
//     config.data_directory = temp_dir.path().to_str().unwrap().to_string();
//     config.enable_embeddings = true;

//     // Use SurrealDB kv-mem backend — same storage path as production daemon
//     config.storage_backend = StorageBackendType::SurrealDB;
//     config.surrealdb_endpoint = Some("mem://".to_string());
//     config.surrealdb_namespace = Some("test".to_string());
//     config.surrealdb_database = Some("test".to_string());

//     let memory = Arc::new(
//         ConversationMemorySystem::new(config)
//             .await
//             .expect("failed to create memory system with SurrealDB kv-mem"),
//     );

//     let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
//     let addr = listener.local_addr().unwrap();

//     let service = PcxGrpcService::new(memory);
//     tokio::spawn(async move {
//         tonic::transport::Server::builder()
//             .add_service(service.into_server())
//             .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
//             .await
//             .unwrap();
//     });

//     // Wait for server to be ready
//     tokio::time::sleep(std::time::Duration::from_millis(50)).await;

//     let client = PostCortexClient::connect(format!("http://{addr}"))
//         .await
//         .expect("failed to connect to gRPC server");

//     (client, temp_dir)
// }

// /// Create a session and return its ID.
// async fn create_session(client: &mut PostCortexClient<Channel>, name: &str) -> String {
//     let resp = client
//         .create_session(CreateSessionRequest {
//             name: name.to_string(),
//             description: String::new(),
//         })
//         .await
//         .expect("create session failed")
//         .into_inner();
//     assert!(!resp.session_id.is_empty());
//     resp.session_id
// }

// fn make_update_request(
//     session_id: &str,
//     interaction_type: &str,
//     title: &str,
//     description: &str,
// ) -> UpdateContextRequest {
//     UpdateContextRequest {
//         session_id: session_id.to_string(),
//         interaction_type: interaction_type.to_string(),
//         content: Some(ContextContent {
//             title: title.to_string(),
//             description: description.to_string(),
//             details: Vec::new(),
//             examples: Vec::new(),
//             implications: Vec::new(),
//             code_ref: None,
//         }),
//         source_ref: None,
//     }
// }

// // ============================================================================
// // Test: UpdateContext stores correct interaction_type and content,
// //       QueryContext returns them faithfully.
// // ============================================================================

// #[serial]
// #[tokio::test]
// async fn test_update_then_query_preserves_type_and_content() {
//     let (mut client, _tmp) = setup_grpc().await;
//     let session_id = create_session(&mut client, "type-preservation").await;

//     // Store a decision
//     let resp = client
//         .update_context(UpdateContextRequest {
//             session_id: session_id.clone(),
//             interaction_type: "decision_made".to_string(),
//             content: Some(ContextContent {
//                 title: "Use PostgreSQL".to_string(),
//                 description: "Decided to use PostgreSQL 15 with deadpool connection pooling."
//                     .to_string(),
//                 details: vec!["Max 20 connections".to_string()],
//                 examples: Vec::new(),
//                 implications: vec!["Need to set up migrations".to_string()],
//                 code_ref: None,
//             }),
//             source_ref: None,
//         })
//         .await
//         .expect("update_context failed")
//         .into_inner();
//     assert!(resp.success, "update should succeed");
//     assert!(!resp.update_id.is_empty(), "update_id should be non-empty");

//     // Query back — no filter
//     let query_resp = client
//         .query_context(QueryContextRequest {
//             session_id: session_id.clone(),
//             interaction_type: String::new(), // all types
//             limit: 10,
//             after_unix: 0,
//         })
//         .await
//         .expect("query_context failed")
//         .into_inner();

//     assert_eq!(query_resp.total, 1, "should have exactly 1 update");
//     let entry = &query_resp.updates[0];

//     // Verify interaction type is DecisionMade (not ConceptDefined!)
//     assert!(
//         entry.interaction_type.contains("DecisionMade"),
//         "interaction_type should be DecisionMade, got: {}",
//         entry.interaction_type
//     );

//     // Verify content preserved
//     let content = entry.content.as_ref().expect("content should be present");
//     assert_eq!(content.title, "Use PostgreSQL");
//     assert!(
//         content.description.contains("PostgreSQL 15"),
//         "description should contain 'PostgreSQL 15', got: {}",
//         content.description
//     );
//     assert!(
//         content.details.contains(&"Max 20 connections".to_string()),
//         "details should be preserved"
//     );
//     assert!(
//         content
//             .implications
//             .contains(&"Need to set up migrations".to_string()),
//         "implications should be preserved"
//     );
// }

// // ============================================================================
// // Test: All interaction types map correctly through the round-trip.
// // ============================================================================

// #[serial]
// #[tokio::test]
// async fn test_all_interaction_types_roundtrip() {
//     let (mut client, _tmp) = setup_grpc().await;
//     let session_id = create_session(&mut client, "all-types").await;

//     let cases = vec![
//         ("decision_made", "DecisionMade"),
//         ("problem_solved", "ProblemSolved"),
//         ("code_change", "CodeChanged"),
//         ("qa", "QuestionAnswered"),
//         ("requirement_added", "RequirementAdded"),
//         ("concept_defined", "ConceptDefined"),
//     ];

//     for (input_type, _) in &cases {
//         client
//             .update_context(make_update_request(
//                 &session_id,
//                 input_type,
//                 &format!("Test {input_type}"),
//                 &format!("Description for {input_type}"),
//             ))
//             .await
//             .unwrap_or_else(|e| panic!("update_context for {input_type} failed: {e}"));
//     }

//     // Query all
//     let resp = client
//         .query_context(QueryContextRequest {
//             session_id: session_id.clone(),
//             interaction_type: String::new(),
//             limit: 20,
//             after_unix: 0,
//         })
//         .await
//         .expect("query_context failed")
//         .into_inner();

//     assert_eq!(
//         resp.total as usize,
//         cases.len(),
//         "should have all {} updates",
//         cases.len()
//     );

//     for (i, (input_type, expected_type)) in cases.iter().enumerate() {
//         let entry = &resp.updates[i];
//         assert!(
//             entry.interaction_type.contains(expected_type),
//             "entry {i} ({input_type}): expected type containing '{expected_type}', got '{}'",
//             entry.interaction_type
//         );
//         let content = entry.content.as_ref().unwrap();
//         assert_eq!(
//             content.title,
//             format!("Test {input_type}"),
//             "entry {i}: title mismatch"
//         );
//     }
// }

// // ============================================================================
// // Test: QueryContext filter by interaction_type works.
// // ============================================================================

// #[serial]
// #[tokio::test]
// async fn test_query_context_filters_by_type() {
//     let (mut client, _tmp) = setup_grpc().await;
//     let session_id = create_session(&mut client, "filter-test").await;

//     // Store one decision and one concept
//     for typ in &["decision_made", "concept_defined"] {
//         client
//             .update_context(make_update_request(
//                 &session_id,
//                 typ,
//                 &format!("Entry {typ}"),
//                 &format!("Desc {typ}"),
//             ))
//             .await
//             .unwrap();
//     }

//     // Filter for DecisionMade only
//     let resp = client
//         .query_context(QueryContextRequest {
//             session_id: session_id.clone(),
//             interaction_type: "DecisionMade".to_string(),
//             limit: 10,
//             after_unix: 0,
//         })
//         .await
//         .expect("query_context failed")
//         .into_inner();

//     assert_eq!(resp.total, 1, "should have exactly 1 decision");
//     assert!(resp.updates[0].interaction_type.contains("DecisionMade"));
// }

// // ============================================================================
// // Test: BulkUpdateContext also produces correct types queryable by QueryContext.
// // ============================================================================

// #[serial]
// #[tokio::test]
// async fn test_bulk_update_then_query() {
//     let (mut client, _tmp) = setup_grpc().await;
//     let session_id = create_session(&mut client, "bulk-test").await;

//     let resp = client
//         .bulk_update_context(BulkUpdateContextRequest {
//             session_id: session_id.clone(),
//             updates: vec![
//                 ContextUpdateItem {
//                     interaction_type: "problem_solved".to_string(),
//                     content: Some(ContextContent {
//                         title: "Fixed N+1".to_string(),
//                         description: "Used JOIN instead of lazy loading".to_string(),
//                         details: Vec::new(),
//                         examples: Vec::new(),
//                         implications: Vec::new(),
//                         code_ref: None,
//                     }),
//                 },
//                 ContextUpdateItem {
//                     interaction_type: "code_change".to_string(),
//                     content: Some(ContextContent {
//                         title: "Added retry middleware".to_string(),
//                         description: "Exponential backoff with jitter".to_string(),
//                         details: Vec::new(),
//                         examples: Vec::new(),
//                         implications: Vec::new(),
//                         code_ref: None,
//                     }),
//                 },
//             ],
//         })
//         .await
//         .expect("bulk_update failed")
//         .into_inner();

//     assert_eq!(resp.success_count, 2);
//     assert_eq!(resp.failure_count, 0);

//     // Query all back
//     let query = client
//         .query_context(QueryContextRequest {
//             session_id,
//             interaction_type: String::new(),
//             limit: 10,
//             after_unix: 0,
//         })
//         .await
//         .expect("query failed")
//         .into_inner();

//     assert_eq!(query.total, 2);

//     let types: Vec<&str> = query
//         .updates
//         .iter()
//         .map(|u| u.interaction_type.as_str())
//         .collect();
//     assert!(
//         types.iter().any(|t| t.contains("ProblemSolved")),
//         "should have ProblemSolved, got: {types:?}"
//     );
//     assert!(
//         types.iter().any(|t| t.contains("CodeChanged")),
//         "should have CodeChanged, got: {types:?}"
//     );

//     // Verify content
//     let n1_entry = query
//         .updates
//         .iter()
//         .find(|u| u.interaction_type.contains("ProblemSolved"))
//         .unwrap();
//     let content = n1_entry.content.as_ref().unwrap();
//     assert_eq!(content.title, "Fixed N+1");
//     assert!(content.description.contains("JOIN"));
// }

// // ============================================================================
// // Test: Full storage round-trip — write → query_context → semantic_search →
// //       structured_summary. Verifies data is persisted and indexed, not just
// //       sitting in memory.
// // ============================================================================

// #[serial]
// #[tokio::test]
// async fn test_full_storage_roundtrip() {
//     let (mut client, _tmp) = setup_grpc().await;
//     let session_id = create_session(&mut client, "storage-roundtrip").await;

//     // ── Write 3 entries of different types ──
//     let entries = vec![
//         ("decision_made", "Use RocksDB for storage", "Chose RocksDB over SQLite for embedded KV. Supports column families, compression, and atomic batches."),
//         ("problem_solved", "Fixed deadlock in worker pool", "Workers were holding read locks while requesting write locks on the same mutex. Switched to lock-free queue with crossbeam."),
//         ("concept_defined", "Event sourcing pattern", "All state changes stored as immutable events. Current state derived by replaying events. Enables audit log and temporal queries."),
//     ];

//     for (typ, title, desc) in &entries {
//         let resp = client
//             .update_context(UpdateContextRequest {
//                 session_id: session_id.clone(),
//                 interaction_type: typ.to_string(),
//                 content: Some(ContextContent {
//                     title: title.to_string(),
//                     description: desc.to_string(),
//                     details: Vec::new(),
//                     examples: Vec::new(),
//                     implications: Vec::new(),
//                     code_ref: None,
//                 }),
//                 source_ref: None,
//             })
//             .await
//             .unwrap_or_else(|e| panic!("update_context for {typ} failed: {e}"))
//             .into_inner();
//         assert!(resp.success, "update for {typ} should succeed");
//     }

//     // ── Verify via query_context: correct types and content ──
//     let qc = client
//         .query_context(QueryContextRequest {
//             session_id: session_id.clone(),
//             interaction_type: String::new(),
//             limit: 10,
//             after_unix: 0,
//         })
//         .await
//         .expect("query_context failed")
//         .into_inner();

//     assert_eq!(qc.total, 3, "query_context should return 3 entries");

//     // Check each type is present with correct content
//     let decision = qc.updates.iter().find(|u| u.interaction_type.contains("DecisionMade"))
//         .expect("should find DecisionMade entry");
//     let dc = decision.content.as_ref().unwrap();
//     assert_eq!(dc.title, "Use RocksDB for storage");
//     assert!(dc.description.contains("RocksDB"), "decision desc should contain RocksDB");
//     assert!(dc.description.contains("column families"), "decision desc should contain column families");

//     let problem = qc.updates.iter().find(|u| u.interaction_type.contains("ProblemSolved"))
//         .expect("should find ProblemSolved entry");
//     let pc = problem.content.as_ref().unwrap();
//     assert_eq!(pc.title, "Fixed deadlock in worker pool");
//     assert!(pc.description.contains("crossbeam"), "problem desc should contain crossbeam");

//     let concept = qc.updates.iter().find(|u| u.interaction_type.contains("ConceptDefined"))
//         .expect("should find ConceptDefined entry");
//     let cc = concept.content.as_ref().unwrap();
//     assert_eq!(cc.title, "Event sourcing pattern");
//     assert!(cc.description.contains("immutable events"), "concept desc should contain immutable events");

//     // ── Verify via semantic_search: data reached the vector index ──
//     // Vectorize first
//     client
//         .vectorize_session(VectorizeSessionRequest {
//             session_id: session_id.clone(),
//         })
//         .await
//         .expect("vectorize_session failed");

//     // Search for RocksDB → should find relevant results from this session
//     // Note: semantic_search returns entity-graph-derived content, not raw update text
//     let search_resp = client
//         .semantic_search(SemanticSearchRequest {
//             query: "RocksDB embedded key-value storage".to_string(),
//             max_results: 5,
//             session_id: session_id.clone(),
//             min_score: 0.0,
//         })
//         .await
//         .expect("semantic_search failed")
//         .into_inner();

//     assert!(
//         search_resp.total_matches >= 1,
//         "semantic_search should find at least 1 result, got {}",
//         search_resp.total_matches
//     );
//     // Verify the top result is from our session and mentions storage-related terms
//     let all_content: String = search_resp.results.iter().map(|r| r.content.clone()).collect::<Vec<_>>().join(" ");
//     assert!(
//         all_content.to_lowercase().contains("rocksdb")
//             || all_content.to_lowercase().contains("storage")
//             || all_content.to_lowercase().contains("decision"),
//         "search results should relate to our stored data, got: {all_content}"
//     );

//     // Search for deadlock → should find relevant results
//     let deadlock_resp = client
//         .semantic_search(SemanticSearchRequest {
//             query: "deadlock worker pool lock-free crossbeam".to_string(),
//             max_results: 5,
//             session_id: session_id.clone(),
//             min_score: 0.0,
//         })
//         .await
//         .expect("semantic_search for deadlock failed")
//         .into_inner();

//     assert!(
//         deadlock_resp.total_matches >= 1,
//         "should find results for deadlock query, got {} matches",
//         deadlock_resp.total_matches
//     );

//     // ── Verify via structured_summary: summary reflects stored data ──
//     let summary_resp = client
//         .get_structured_summary(GetStructuredSummaryRequest {
//             session_id: session_id.clone(),
//             compact: Some(false),
//             decisions_limit: None,
//             entities_limit: None,
//             questions_limit: None,
//             concepts_limit: None,
//             min_confidence: None,
//         })
//         .await
//         .expect("get_structured_summary failed")
//         .into_inner();

//     assert!(!summary_resp.text.is_empty(), "summary should not be empty");

//     // ── Verify via session_statistics: counts match ──
//     let stats_resp = client
//         .get_session_statistics(GetSessionStatisticsRequest {
//             session_id: session_id.clone(),
//         })
//         .await
//         .expect("get_session_statistics failed")
//         .into_inner();

//     assert!(!stats_resp.text.is_empty(), "stats should not be empty");
// }

// // ============================================================================
// // Test: Negative — searching for unrelated content should NOT return our data.
// // ============================================================================

// #[serial]
// #[tokio::test]
// async fn test_search_does_not_return_unrelated() {
//     let (mut client, _tmp) = setup_grpc().await;
//     let session_id = create_session(&mut client, "negative-search").await;

//     // Store something specific
//     client
//         .update_context(UpdateContextRequest {
//             session_id: session_id.clone(),
//             interaction_type: "decision_made".to_string(),
//             content: Some(ContextContent {
//                 title: "Use Rust for backend".to_string(),
//                 description: "Chose Rust for its memory safety and zero-cost abstractions."
//                     .to_string(),
//                 details: Vec::new(),
//                 examples: Vec::new(),
//                 implications: Vec::new(),
//                 code_ref: None,
//             }),
//             source_ref: None,
//         })
//         .await
//         .unwrap();

//     // Vectorize
//     client
//         .vectorize_session(VectorizeSessionRequest {
//             session_id: session_id.clone(),
//         })
//         .await
//         .unwrap();

//     // Search for something related → should score higher than unrelated
//     let related = client
//         .semantic_search(SemanticSearchRequest {
//             query: "Rust programming language memory safety".to_string(),
//             max_results: 1,
//             session_id: session_id.clone(),
//             min_score: 0.0,
//         })
//         .await
//         .expect("related search failed")
//         .into_inner();

//     let unrelated = client
//         .semantic_search(SemanticSearchRequest {
//             query: "kubernetes helm chart deployment yaml".to_string(),
//             max_results: 1,
//             session_id: session_id.clone(),
//             min_score: 0.0,
//         })
//         .await
//         .expect("unrelated search failed")
//         .into_inner();

//     // Related query should score higher than unrelated query
//     if !related.results.is_empty() && !unrelated.results.is_empty() {
//         let related_score = related.results[0].score;
//         let unrelated_score = unrelated.results[0].score;
//         assert!(
//             related_score >= unrelated_score,
//             "related query ({related_score:.3}) should score >= unrelated ({unrelated_score:.3})"
//         );
//     }
// }
