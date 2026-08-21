//! `GET /api/services/map` — the service topology.
//!
//! One endpoint, two readings: without `service` it is the fleet graph, with `service` it is that
//! service's focused map. They are the same result narrowed, not two features — a fleet graph and a
//! focused map that disagreed about an edge's error count would be worse than either alone.
//!
//! **The `service` param must not reach the grammar query.** Scoping the SCAN to one service would
//! delete exactly the parent spans the join needs, leaving that service's spans with nobody to
//! attribute them to — the map would come back empty and look like "nothing calls this". Narrowing
//! happens after the edges exist, inside `photon_query::service_map`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::{MapEdge, MapNode};

use crate::query_params::build_span_query_request;
use crate::red::window_seconds;
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct MapParams {
    /// Caller's own grammar filter (the UI's Site/environment context terms). Applied to the scan,
    /// unlike `service`.
    #[serde(default)]
    q: String,
    /// Narrow to edges touching this service, in both directions. Absent = the whole fleet.
    #[serde(default)]
    service: Option<String>,
    start: String,
    end: String,
}

/// `GET /api/services/map?start&end&q&service`.
pub(crate) async fn service_map(
    State(state): State<AppState>,
    Query(p): Query<MapParams>,
) -> Response {
    let req = match build_span_query_request(
        p.q.trim(),
        &p.start,
        &p.end,
        "recent",
        0,
        0,
        state.span_query.promoted_attributes(),
    ) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let (start_ns, end_ns) = (req.start_ts_nanos, req.end_ts_nanos);

    let focus = p.service.as_deref().filter(|s| !s.is_empty());
    let map = match state.span_query.service_map(req, focus).await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    let secs = window_seconds(start_ns, end_ns);
    Json(json!({
        "window": { "start": start_ns.to_string(), "end": end_ns.to_string() },
        "service": focus,
        "nodes": map.nodes.iter().map(|n| node_json(n, secs)).collect::<Vec<_>>(),
        "edges": map.edges.iter().map(|e| edge_json(e, secs)).collect::<Vec<_>>(),
        // Reported, not hidden: a truncated map that looks complete is worse than no map.
        "scanned": map.scanned,
        "capped": map.capped,
    }))
    .into_response()
}

/// Rates are derived here, not in the engine — same split as `/api/red` and `dependencies`, which
/// keeps `photon-query` window-agnostic beyond pruning.
fn edge_json(e: &MapEdge, window_secs: f64) -> Value {
    json!({
        "from": e.from,
        "to": e.to,
        "count": e.count,
        "rate": rate(e.count, window_secs),
        "error_count": e.error_count,
        "error_rate": ratio(e.error_count, e.count),
        // Nanosecond STRINGS (JS-safe), matching every other percentile on this surface.
        "p50": e.p50.to_string(),
        "p95": e.p95.to_string(),
        "p99": e.p99.to_string(),
    })
}

fn node_json(n: &MapNode, window_secs: f64) -> Value {
    json!({
        "service": n.service,
        "count": n.count,
        "rate": rate(n.count, window_secs),
        "error_count": n.error_count,
        "error_rate": ratio(n.error_count, n.count),
        "roots": n.roots,
        // A service with root spans is where work enters the system — what a layout puts on the left.
        "is_entry": n.roots > 0,
    })
}

fn rate(count: u64, window_secs: f64) -> f64 {
    if window_secs > 0.0 {
        count as f64 / window_secs
    } else {
        0.0
    }
}

fn ratio(part: u64, whole: u64) -> f64 {
    if whole > 0 {
        part as f64 / whole as f64
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    async fn get(uri: &str) -> (StatusCode, Value) {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn the_map_requires_a_session() {
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/services/map?start=0&end=100")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_empty_server_returns_an_empty_map_that_says_it_scanned_nothing() {
        let (status, v) = get("/api/services/map?start=0&end=100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["nodes"], json!([]));
        assert_eq!(v["edges"], json!([]));
        assert_eq!(v["scanned"], json!(0));
        assert_eq!(v["capped"], json!(false));
        assert_eq!(v["service"], Value::Null);
    }

    #[tokio::test]
    async fn a_service_param_is_echoed_as_the_focus() {
        let (status, v) = get("/api/services/map?start=0&end=100&service=api").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["service"], json!("api"));
    }

    #[tokio::test]
    async fn a_blank_service_param_is_the_fleet_map_not_a_service_named_empty() {
        let (_, v) = get("/api/services/map?start=0&end=100&service=").await;
        assert_eq!(v["service"], Value::Null);
    }

    #[tokio::test]
    async fn a_malformed_query_is_a_400_with_an_offset() {
        let (status, v) = get("/api/services/map?start=0&end=100&q=ok%20%3Abad").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(v["offset"].is_number());
    }

    #[test]
    fn a_rate_over_a_zero_length_window_is_zero_not_infinity() {
        assert_eq!(rate(10, 0.0), 0.0);
        assert_eq!(ratio(1, 0), 0.0);
    }
}
