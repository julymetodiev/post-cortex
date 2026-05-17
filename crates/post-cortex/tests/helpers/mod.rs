// Test helpers for in-memory HTTP testing without TCP ports

use axum::Router;
use axum::body::{Body, to_bytes};
use hyper::{Request, Response, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

/// In-memory test application that runs Axum router without TCP
///
/// This helper eliminates the need for real TCP ports in tests,
/// making them faster, more reliable, and eliminating port conflicts.
///
/// # Example
///
/// ```rust
/// let app = TestApp::new(router);
/// let response = app.get("/health").await;
/// assert_eq!(response.status(), StatusCode::OK);
/// ```
pub struct TestApp {
    pub router: Router,
}

impl TestApp {
    /// Create a new test app with the given router
    pub fn new(router: Router) -> Self {
        Self { router }
    }

    /// Make an HTTP request without network stack
    ///
    /// Uses `tower::ServiceExt::oneshot` to process the request
    /// directly through the router without binding to a port.
    pub async fn request(&self, req: Request<Body>) -> Response<Body> {
        self.router
            .clone()
            .oneshot(req)
            .await
            .expect("Request failed")
    }

    /// Helper: Make a GET request
    pub async fn get(&self, uri: &str) -> Response<Body> {
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        self.request(req).await
    }

    /// Helper: Make a POST request with JSON body
    pub async fn post_json(&self, uri: &str, json: Value) -> Response<Body> {
        let body = serde_json::to_string(&json).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        self.request(req).await
    }

    /// Helper: Extract JSON body from response
    pub async fn json_body(response: Response<Body>) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Helper: Extract body as string
    pub async fn body_string(response: Response<Body>) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, routing::get};

    #[tokio::test]
    async fn test_app_get_request() {
        // Create a simple router
        let router = Router::new().route("/test", get(|| async { "Hello, World!" }));

        let app = TestApp::new(router);
        let response = app.get("/test").await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = TestApp::body_string(response).await;
        assert_eq!(body, "Hello, World!");
    }

    #[tokio::test]
    async fn test_app_post_json() {
        use serde_json::json;

        // Router that echoes JSON
        async fn echo_json(Json(payload): Json<Value>) -> Json<Value> {
            Json(payload)
        }

        let router = Router::new().route("/echo", axum::routing::post(echo_json));

        let app = TestApp::new(router);
        let test_json = json!({"message": "test"});
        let response = app.post_json("/echo", test_json.clone()).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = TestApp::json_body(response).await;
        assert_eq!(body, test_json);
    }
}
