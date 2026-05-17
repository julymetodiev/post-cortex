// Copyright (c) 2025 Julius ML
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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Lock-free Server-Sent Events (SSE) broadcaster
//!
//! Uses DashMap and tokio channels for zero-blocking event broadcasting.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

/// SSE event that can be broadcasted to clients
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    /// Unique event identifier.
    pub id: String,
    /// Event type label.
    pub event_type: String,
    /// JSON event payload.
    pub data: serde_json::Value,
}

/// Lock-free SSE broadcaster using DashMap
///
/// All operations are non-blocking using DashMap for client tracking
/// and tokio unbounded channels for message passing.
pub struct SSEBroadcaster {
    /// Lock-free client registry
    clients: Arc<DashMap<Uuid, UnboundedSender<SseEvent>>>,

    /// Atomic event counter
    event_counter: Arc<AtomicU64>,

    /// Total clients counter
    total_clients: Arc<AtomicU64>,
}

impl SSEBroadcaster {
    /// Create a new broadcaster with no connected clients.
    pub fn new() -> Self {
        Self {
            clients: Arc::new(DashMap::new()),
            event_counter: Arc::new(AtomicU64::new(0)),
            total_clients: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Register a new client (lock-free)
    pub fn register_client(&self, id: Uuid) -> UnboundedReceiver<SseEvent> {
        let (tx, rx) = unbounded_channel();
        self.clients.insert(id, tx);
        self.total_clients.fetch_add(1, Ordering::Relaxed);
        rx
    }

    /// Unregister a client (lock-free)
    pub fn unregister_client(&self, id: &Uuid) {
        if self.clients.remove(id).is_some() {
            self.total_clients.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Broadcast event to all clients (lock-free)
    pub fn broadcast(&self, event: SseEvent) {
        self.event_counter.fetch_add(1, Ordering::Relaxed);

        // Lock-free iteration over DashMap
        self.clients.iter().for_each(|entry| {
            // Fire-and-forget send (if client channel is full/closed, it gets dropped)
            let _ = entry.value().send(event.clone());
        });
    }

    /// Send event to specific client (lock-free)
    pub fn send_to_client(&self, client_id: &Uuid, event: SseEvent) -> Result<(), String> {
        self.clients
            .get(client_id)
            .ok_or_else(|| format!("Client {} not found", client_id))?
            .send(event)
            .map_err(|e| format!("Failed to send to client: {}", e))
    }

    /// Get active client count (atomic read)
    pub fn active_clients(&self) -> u64 {
        self.total_clients.load(Ordering::Relaxed)
    }

    /// Get total events broadcasted (atomic read)
    pub fn total_events(&self) -> u64 {
        self.event_counter.load(Ordering::Relaxed)
    }
}

impl Default for SSEBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sse_broadcaster_registration() {
        let broadcaster = SSEBroadcaster::new();

        let client1 = Uuid::new_v4();
        let mut rx1 = broadcaster.register_client(client1);

        assert_eq!(broadcaster.active_clients(), 1);

        // Broadcast event
        let event = SseEvent {
            id: "1".to_string(),
            event_type: "test".to_string(),
            data: serde_json::json!({"message": "hello"}),
        };

        broadcaster.broadcast(event.clone());

        // Receive event
        let received = rx1.recv().await.unwrap();
        assert_eq!(received.id, "1");
        assert_eq!(received.event_type, "test");

        // Unregister
        broadcaster.unregister_client(&client1);
        assert_eq!(broadcaster.active_clients(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_sse_operations() {
        let broadcaster = Arc::new(SSEBroadcaster::new());

        // Register 50 clients concurrently
        let mut handles = vec![];
        for _ in 0..50 {
            let bc = broadcaster.clone();
            let handle = tokio::spawn(async move {
                let id = Uuid::new_v4();
                let mut rx = bc.register_client(id);

                // Receive one event
                let event = rx.recv().await;
                assert!(event.is_some());

                bc.unregister_client(&id);
            });
            handles.push(handle);
        }

        // Wait a bit for clients to register
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Broadcast event
        broadcaster.broadcast(SseEvent {
            id: "broadcast".to_string(),
            event_type: "test".to_string(),
            data: serde_json::json!({}),
        });

        // Wait for all clients
        for handle in handles {
            handle.await.unwrap();
        }

        // All clients should be unregistered
        assert_eq!(broadcaster.active_clients(), 0);
        assert_eq!(broadcaster.total_events(), 1);
    }
}
