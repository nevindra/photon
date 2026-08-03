//! Federation status seam: `GET /api/federation/status` reports the tenant-side summary
//! pusher's/tee's live push telemetry to the UI. Defined here — not in `photon-server`, which
//! owns the actual pusher/tee — because `photon-api` cannot depend on `photon-server`;
//! `photon-server` implements [`FederationStatus`] as a newtype over its `FederationConfig` +
//! `FederationStats`, same shape as `ReplicationStatus` (`usage.rs`).

use axum::extract::State;
use axum::Json;

use crate::AppState;

/// Server-supplied view of the tenant-side federation pusher/tee.
pub trait FederationStatus: Send + Sync {
    /// `None` = federation disabled (`[federation]` absent from config).
    fn snapshot(&self) -> Option<FederationStatusSnapshot>;
}

#[derive(serde::Serialize, Clone, PartialEq, Debug)]
pub struct FederationStatusSnapshot {
    pub mode: String, // "summary" | "full"
    pub endpoint: String,
    pub last_push_ms: i64, // 0 = never
    pub last_error: Option<String>,
    pub pushed: u64,
    pub dropped: u64,
    pub queued: u64,
}

pub(crate) async fn status(State(st): State<AppState>) -> Json<serde_json::Value> {
    match st.federation.as_ref().and_then(|f| f.snapshot()) {
        Some(s) => Json(serde_json::json!({ "enabled": true, "status": s })),
        None => Json(serde_json::json!({ "enabled": false, "status": null })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    struct FakeStatus(Option<FederationStatusSnapshot>);

    impl FederationStatus for FakeStatus {
        fn snapshot(&self) -> Option<FederationStatusSnapshot> {
            self.0.clone()
        }
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn get_status(app: axum::Router) -> serde_json::Value {
        let cookie = crate::session_cookie(&app).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/federation/status")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        body_json(resp).await
    }

    #[tokio::test]
    async fn disabled_reports_enabled_false() {
        let app = crate::test_server()
            .with_federation_status(None)
            .into_router();
        let v = get_status(app).await;
        assert_eq!(v["enabled"], false);
        assert!(v["status"].is_null());
    }

    #[tokio::test]
    async fn enabled_reports_snapshot() {
        let snap = FederationStatusSnapshot {
            mode: "full".into(),
            endpoint: "https://central.example.com".into(),
            last_push_ms: 1_751_000_000_000,
            last_error: None,
            pushed: 42,
            dropped: 1,
            queued: 3,
        };
        let app = crate::test_server()
            .with_federation_status(Some(std::sync::Arc::new(FakeStatus(Some(snap.clone())))))
            .into_router();
        let v = get_status(app).await;
        assert_eq!(v["enabled"], true);
        assert_eq!(v["status"]["mode"], "full");
        assert_eq!(v["status"]["pushed"], 42);
        assert_eq!(v["status"]["queued"], 3);
    }
}
