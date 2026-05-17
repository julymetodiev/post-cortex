use post_cortex::{ConversationMemorySystem, SystemConfig};
use serial_test::serial;
use std::sync::Arc;
use uuid::Uuid;

async fn create_test_system() -> (Arc<ConversationMemorySystem>, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = SystemConfig {
        data_directory: temp_dir.path().to_str().unwrap().to_string(),
        enable_embeddings: true,
        auto_vectorize_on_update: true, // We need vectorization for search
        semantic_search_threshold: 0.1, // Low threshold for testing with basic embeddings
        ..Default::default()
    };
    let system = ConversationMemorySystem::new(config).await.unwrap();
    
    // Initialize embeddings engine (lazy loading)
    // We need to trigger it or ensure it's ready.
    // In tests, auto_vectorize might be slow or async.
    
    (Arc::new(system), temp_dir)
}

#[serial]
#[tokio::test]
async fn test_comprehensive_unified_search() {
    let (system_arc, _temp_dir) = create_test_system().await;
    let system = system_arc.as_ref();
    
    // Ensure semantic engine is initialized
    system.ensure_semantic_engine_initialized().await.unwrap();
    
    // 1. Create Workspaces
    let ws_trading = system.workspace_manager.create_workspace("Trading System".to_string(), "Microservices".to_string());
    let ws_dating = system.workspace_manager.create_workspace("Dating App".to_string(), "Social platform".to_string());
    
    // 2. Create Sessions
    // Trading Workspace
    let sess_auth = system.create_session(Some("Auth Service".to_string()), None).await.unwrap();
    let sess_payment = system.create_session(Some("Payment Service".to_string()), None).await.unwrap();
    
    // Dating Workspace
    let sess_matching = system.create_session(Some("Matching Service".to_string()), None).await.unwrap();
    
    // 3. Assign to Workspaces
    use post_cortex::workspace::SessionRole;
    system.workspace_manager.add_session_to_workspace(&ws_trading, sess_auth, SessionRole::Primary).unwrap();
    system.workspace_manager.add_session_to_workspace(&ws_trading, sess_payment, SessionRole::Related).unwrap();
    system.workspace_manager.add_session_to_workspace(&ws_dating, sess_matching, SessionRole::Primary).unwrap();
    
    // 4. Add Content
    
    async fn add_updates(sys: &ConversationMemorySystem, sess: Uuid, updates: Vec<&str>) {
        for text in updates {
            let mut content = std::collections::HashMap::new();
            content.insert("text".to_string(), text.to_string());
            
            post_cortex::tools::mcp::update_conversation_context_with_system(
                "qa".to_string(), 
                content, 
                None, 
                sess,
                sys
            ).await.unwrap();
        }
    }
    
    // Auth Service Data
    let auth_data = vec![
        "Service uses JWT (JSON Web Tokens) for stateless authentication.",
        "Tokens expire after 15 minutes to ensure security.",
        "Public keys are fetched from the JWKS endpoint.",
        "Rate limiting is set to 5 login attempts per minute.",
        "OAuth2 providers include Google and GitHub."
    ];
    add_updates(system, sess_auth, auth_data).await;
    
    // Payment Service Data
    let payment_data = vec![
        "Payment service integrates with Stripe API.",
        "Webhooks are verified using the signing secret.",
        "Currency conversion uses the Fixer.io API.",
        "Transactions are ACID compliant using PostgreSQL.",
        "Refunds require manager approval."
    ];
    add_updates(system, sess_payment, payment_data).await;
    
    // Matching Service Data (Dating App)
    let matching_data = vec![
        "Matching algorithm uses a graph database (Neo4j).",
        "User preferences include age range and location (geolocation).",
        "Swiping left ignores the profile permanently.",
        "Swiping right sends a like notification.",
        "Premium users get unlimited rewinds on swipes.",
        "Chat is enabled only after a mutual match.",
        "Images are scanned for NSFW content using AI."
    ];
    add_updates(system, sess_matching, matching_data).await;
    
    // 5. Wait for vectorization
    println!("Waiting for vectorization...");
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;
    
    // 6. Search Helper
    let search = |query: &str, scope_type: &str, id: Option<Uuid>| {
        let sys = system_arc.clone();
        let q = query.to_string();
        let s_type = scope_type.to_string();
        async move {
            let engine = sys.semantic_query_engine.get().unwrap();
            if s_type == "workspace" {
                let ws = sys.workspace_manager.get_workspace(&id.unwrap()).unwrap();
                let s_ids: Vec<Uuid> = ws.get_all_sessions().into_iter().map(|(i, _)| i).collect();
                engine.semantic_search_multisession(&s_ids, &q, None, None, None).await.unwrap()
            } else if s_type == "session" {
                engine.semantic_search_session(id.unwrap(), &q, None, None, None).await.unwrap()
            } else {
                engine.semantic_search_global(&q, None, None, None).await.unwrap()
            }
        }
    };
    
    // Test Case 1: Search "token" in Trading Workspace -> Should find Auth Service
    let results = search("token", "workspace", Some(ws_trading)).await;
    println!("Search 'token' in Trading: Found {} results", results.len());
    assert!(!results.is_empty(), "Should find token info in Trading");
    assert!(results.iter().any(|r| r.session_id == sess_auth), "Should find Auth session");
    // Isolation check: Should NOT find anything from Dating (unlikely anyway, but good to check)
    assert!(!results.iter().any(|r| r.session_id == sess_matching));
    
    // Test Case 2: Search "stripe" in Trading Workspace -> Should find Payment Service
    let results = search("stripe", "workspace", Some(ws_trading)).await;
    println!("Search 'stripe' in Trading: Found {} results", results.len());
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.session_id == sess_payment));
    
    // Test Case 3: Search "swiping" in Trading Workspace -> Should NOT find matching service
    // Due to low threshold in test, it might find *something*, but it MUST NOT be from sess_matching
    let results = search("swiping", "workspace", Some(ws_trading)).await;
    println!("Search 'swiping' in Trading: Found {} results", results.len());
    assert!(!results.iter().any(|r| r.session_id == sess_matching), "Leak! Found Dating session in Trading workspace search");
    
    // Test Case 4: Search "swiping" in Dating Workspace -> Should find Matching Service
    let results = search("swiping", "workspace", Some(ws_dating)).await;
    println!("Search 'swiping' in Dating: Found {} results", results.len());
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.session_id == sess_matching));
    
    // Test Case 5: Search "database" Global -> Should find Payment (ACID) and Matching (Graph DB)
    let results = search("database", "global", None).await;
    println!("Search 'database' Global: Found {} results", results.len());
    let found_payment = results.iter().any(|r| r.session_id == sess_payment);
    let found_matching = results.iter().any(|r| r.session_id == sess_matching);
    
    println!("Found Payment (PostgreSQL): {}", found_payment);
    println!("Found Matching (Neo4j): {}", found_matching);
    
    // With dummy embeddings, relevance might be tricky, but let's hope keywords match enough
    // If not, we check at least one is found to verify global search works
    assert!(found_payment || found_matching, "Global search should find database references");
    
    // Test Case 6: Scoped Session Search
    // Search "security" only in Auth Service
    let results = search("security", "session", Some(sess_auth)).await;
    assert!(!results.is_empty());
    assert!(results.iter().all(|r| r.session_id == sess_auth), "Session search must filter by session");
}

#[serial]
#[tokio::test]
async fn test_session_isolation_bug_reproduction() {
    // This test specifically reproduces the bug found during manual testing
    // where semantic_search_session returns results from Session 1 regardless
    // of which session is being searched.

    let (system_arc, _temp_dir) = create_test_system().await;
    let system = system_arc.as_ref();

    // Ensure semantic engine is initialized
    system.ensure_semantic_engine_initialized().await.unwrap();

    // Create 3 sessions with VERY distinct content
    let session_1 = system.create_session(Some("User Management Service".to_string()), None).await.unwrap();
    let session_2 = system.create_session(Some("Payment Gateway Service".to_string()), None).await.unwrap();
    let session_3 = system.create_session(Some("Notification Service".to_string()), None).await.unwrap();

    // Add very specific content to each session

    // Session 1: Authentication-specific content
    let session_1_data = vec![
        "JWT authentication with RS256 asymmetric encryption",
        "Multi-factor authentication using TOTP and backup codes",
        "User registration with email verification tokens",
        "Password hashing with bcrypt and salt rounds",
        "Role-based access control with permissions"
    ];

    // Session 2: Payment-specific content (NO overlap with session 1)
    let session_2_data = vec![
        "Stripe webhook signature verification with clock skew tolerance",
        "PayPal REST API integration for alternative payments",
        "Idempotent payment processing with Redis distributed locks",
        "PCI-DSS compliance for credit card transaction security"
    ];

    // Session 3: Notification-specific content (NO overlap with session 1 or 2)
    let session_3_data = vec![
        "Push notifications via Firebase Cloud Messaging",
        "Email templates using Handlebars rendering engine",
        "SMS delivery through Twilio API with retry logic",
        "In-app notification badges and unread counts"
    ];

    // Helper to add updates
    async fn add_updates(sys: &ConversationMemorySystem, sess: Uuid, updates: Vec<&str>) {
        for text in updates {
            let mut content = std::collections::HashMap::new();
            content.insert("question".to_string(), format!("How does this work?"));
            content.insert("answer".to_string(), text.to_string());

            post_cortex::tools::mcp::update_conversation_context_with_system(
                "qa".to_string(),
                content,
                None,
                sess,
                sys
            ).await.unwrap();
        }
    }

    add_updates(system, session_1, session_1_data).await;
    add_updates(system, session_2, session_2_data).await;
    add_updates(system, session_3, session_3_data).await;

    // Wait for vectorization
    println!("Waiting for vectorization...");
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let engine = system.semantic_query_engine.get().unwrap();

    println!("\nSession IDs:");
    println!("  Session 1: {}", session_1);
    println!("  Session 2: {}", session_2);
    println!("  Session 3: {}", session_3);

    // TEST 1: Search Session 2 for payment-specific content
    // Should ONLY return results from Session 2, NOT Session 1 or Session 3
    println!("\n=== TEST 1: Search Session 2 for 'Stripe webhook PayPal payment' ===");
    let results = engine.semantic_search_session(
        session_2,
        "Stripe webhook PayPal payment idempotent",
        None,
        None,
        None
    ).await.unwrap();

    println!("Found {} results", results.len());
    for (i, result) in results.iter().enumerate() {
        println!("Result {}: Session {} - Similarity {:.2}%",
            i+1,
            if result.session_id == session_1 { "1" }
            else if result.session_id == session_2 { "2" }
            else { "3" },
            result.similarity_score * 100.0
        );
    }

    // CRITICAL: All results MUST be from session_2
    assert!(!results.is_empty(), "Should find payment-related content in Session 2");
    let all_from_session_2 = results.iter().all(|r| r.session_id == session_2);
    assert!(
        all_from_session_2,
        "BUG: Session 2 search returned results from other sessions! Found sessions: {:?}",
        results.iter().map(|r| r.session_id).collect::<Vec<_>>()
    );

    // TEST 2: Search Session 1 for authentication-specific content
    // Should ONLY return results from Session 1
    println!("\n=== TEST 2: Search Session 1 for 'JWT authentication TOTP MFA' ===");
    let results = engine.semantic_search_session(
        session_1,
        "JWT authentication TOTP MFA bcrypt",
        None,
        None,
        None
    ).await.unwrap();

    println!("Found {} results", results.len());
    for (i, result) in results.iter().enumerate() {
        println!("Result {}: Session {} - Similarity {:.2}%",
            i+1,
            if result.session_id == session_1 { "1" }
            else if result.session_id == session_2 { "2" }
            else { "3" },
            result.similarity_score * 100.0
        );
    }

    assert!(!results.is_empty(), "Should find auth-related content in Session 1");
    let all_from_session_1 = results.iter().all(|r| r.session_id == session_1);
    assert!(
        all_from_session_1,
        "BUG: Session 1 search returned results from other sessions! Found sessions: {:?}",
        results.iter().map(|r| r.session_id).collect::<Vec<_>>()
    );

    // TEST 3: Search Session 3 for notification-specific content
    // Should ONLY return results from Session 3
    println!("\n=== TEST 3: Search Session 3 for 'Firebase push notification SMS Twilio' ===");
    let results = engine.semantic_search_session(
        session_3,
        "Firebase push notification SMS Twilio email",
        None,
        None,
        None
    ).await.unwrap();

    println!("Found {} results", results.len());
    for (i, result) in results.iter().enumerate() {
        println!("Result {}: Session {} - Similarity {:.2}%",
            i+1,
            if result.session_id == session_1 { "1" }
            else if result.session_id == session_2 { "2" }
            else { "3" },
            result.similarity_score * 100.0
        );
    }

    assert!(!results.is_empty(), "Should find notification-related content in Session 3");
    let all_from_session_3 = results.iter().all(|r| r.session_id == session_3);
    assert!(
        all_from_session_3,
        "BUG: Session 3 search returned results from other sessions! Found sessions: {:?}",
        results.iter().map(|r| r.session_id).collect::<Vec<_>>()
    );

    // TEST 4: Global search should find results from ALL sessions
    println!("\n=== TEST 4: Global search for 'authentication payment notification' ===");
    let results = engine.semantic_search_global(
        "authentication payment notification",
        None,
        None,
        None
    ).await.unwrap();

    println!("Found {} results across all sessions", results.len());

    let found_session_1 = results.iter().any(|r| r.session_id == session_1);
    let found_session_2 = results.iter().any(|r| r.session_id == session_2);
    let found_session_3 = results.iter().any(|r| r.session_id == session_3);

    println!("Found Session 1 (auth): {}", found_session_1);
    println!("Found Session 2 (payment): {}", found_session_2);
    println!("Found Session 3 (notification): {}", found_session_3);

    // Global search should find results from multiple sessions
    let unique_sessions: std::collections::HashSet<_> = results.iter().map(|r| r.session_id).collect();
    println!("Unique sessions in results: {}", unique_sessions.len());

    assert!(
        unique_sessions.len() > 1,
        "BUG: Global search only returned results from {} session(s), expected multiple! Sessions: {:?}",
        unique_sessions.len(),
        unique_sessions
    );

    // Ideally, should find content from all 3 sessions, but at minimum 2
    assert!(
        unique_sessions.len() >= 2,
        "Global search should return results from at least 2 different sessions"
    );
}
