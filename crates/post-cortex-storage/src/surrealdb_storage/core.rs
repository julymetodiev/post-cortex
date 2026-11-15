// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! Connecting [`SurrealDBStorage`]: sign-in, schema and migrations.

use crate::port_error::Result;
use std::sync::Arc;
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::opt::auth::{Namespace, Root};
use tracing::info;

use crate::surrealdb_storage::SurrealDBStorage;
use crate::surrealdb_storage::connection::{Connection, Opener};

impl SurrealDBStorage {
    /// Create a new SurrealDB storage instance with remote WebSocket connection
    ///
    /// # Arguments
    /// * `endpoint` - WebSocket endpoint (e.g., "localhost:8000" or "ws://localhost:8000")
    /// * `username` - Database username (optional, for authentication)
    /// * `password` - Database password (optional, for authentication)
    ///
    /// # Example
    /// ```bash
    /// docker run -d -p 8000:8000 surrealdb/surrealdb:latest start --user root --pass root
    /// ```
    /// ```rust,ignore
    /// let storage = SurrealDBStorage::new("localhost:8000", Some("root"), Some("root"), None, None).await?;
    /// ```
    ///
    /// Accepts embeddings of any length of at least [`crate::surrealdb_storage::MIN_VECTOR_LEN`];
    /// [`Self::new_with_dimension`] accepts only the embedding model's.
    pub async fn new(
        endpoint: &str,
        username: Option<&str>,
        password: Option<&str>,
        namespace: Option<&str>,
        database: Option<&str>,
    ) -> Result<Self> {
        Self::open(endpoint, username, password, namespace, database, None).await
    }

    /// [`Self::new`] storing only embeddings of exactly `vector_dimension`
    /// floats, as `RealRocksDBStorage::new_with_dimension` does; `0`
    /// behaves like [`Self::new`].
    ///
    /// Runs the schema initialisation and the data migrations (see
    /// [`Self::open_existing`] for a connection that changes nothing).
    pub async fn new_with_dimension(
        endpoint: &str,
        username: Option<&str>,
        password: Option<&str>,
        namespace: Option<&str>,
        database: Option<&str>,
        vector_dimension: usize,
    ) -> Result<Self> {
        let dimension = (vector_dimension > 0).then_some(vector_dimension);
        Self::open(endpoint, username, password, namespace, database, dimension).await
    }

    async fn open(
        endpoint: &str,
        username: Option<&str>,
        password: Option<&str>,
        namespace: Option<&str>,
        database: Option<&str>,
        vector_dimension: Option<usize>,
    ) -> Result<Self> {
        let mut storage = Self::connect(endpoint, username, password, namespace, database).await?;
        storage.vector_dimension = vector_dimension;
        crate::surrealdb_storage::schema::initialize(&storage.db()).await?;
        crate::surrealdb_storage::migrate::migrate(&storage.db(), &storage.name).await?;
        info!("SurrealDBStorage: Remote instance initialized successfully");
        Ok(storage)
    }

    /// Connect to a database a post-cortex daemon already initialised,
    /// without running any schema statement (no `DEFINE`, no `REMOVE`).
    ///
    /// For tools that only read (`pcx export`): they must never change the
    /// schema of a database a daemon is serving, whatever embedding model
    /// they were built with. Fails when the database has no post-cortex
    /// schema (wrong namespace or database name, or never initialised).
    pub async fn open_existing(
        endpoint: &str,
        username: Option<&str>,
        password: Option<&str>,
        namespace: Option<&str>,
        database: Option<&str>,
    ) -> Result<Self> {
        let storage = Self::connect(endpoint, username, password, namespace, database).await?;
        crate::surrealdb_storage::schema::check_initialised(&storage.db(), &storage.name).await?;
        info!(
            "SurrealDBStorage: Opened existing database {} (no schema changes)",
            storage.name
        );
        Ok(storage)
    }

    /// Connect, sign in and select the namespace and database, kept
    /// answering ([`Connection`]). Changes nothing.
    async fn connect(
        endpoint: &str,
        username: Option<&str>,
        password: Option<&str>,
        namespace: Option<&str>,
        database: Option<&str>,
    ) -> Result<Self> {
        // Normalize endpoint (add ws:// if not present and not using other engines)
        let endpoint = if endpoint.contains("://") {
            endpoint.to_string()
        } else {
            format!("ws://{}", endpoint)
        };
        let namespace = namespace.unwrap_or("post_cortex").to_string();
        let database = database.unwrap_or("main").to_string();
        let name = format!("{namespace}/{database}");
        info!(
            "SurrealDBStorage: Connecting to remote SurrealDB at {} ({})",
            endpoint, name
        );
        let credentials = username
            .zip(password)
            .map(|(u, p)| (u.to_string(), p.to_string()));
        let open: Opener = Arc::new(move || {
            let (endpoint, namespace, database, credentials) = (
                endpoint.clone(),
                namespace.clone(),
                database.clone(),
                credentials.clone(),
            );
            Box::pin(
                async move { open_client(&endpoint, credentials, &namespace, &database).await },
            )
        });
        let connection = Connection::open(name.clone(), open).await?;
        Ok(Self {
            connection,
            name,
            vector_dimension: None,
            breaker: Arc::default(),
        })
    }
}

/// A connection to `endpoint`, signed in with `credentials` (`root` at root
/// level, any other user as a namespace user of `namespace`, so the daemon
/// need not hold root), `namespace` and `database` selected.
async fn open_client(
    endpoint: &str,
    credentials: Option<(String, String)>,
    namespace: &str,
    database: &str,
) -> surrealdb::Result<Surreal<Any>> {
    let db = connect(endpoint).await?;
    if let Some((username, password)) = credentials {
        if username == "root" {
            db.signin(Root { username, password }).await?;
        } else {
            db.signin(Namespace {
                namespace: namespace.to_string(),
                username,
                password,
            })
            .await?;
        }
    }
    db.use_ns(namespace).use_db(database).await?;
    Ok(db)
}
