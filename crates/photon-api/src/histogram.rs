//! `GET /api/histogram?query&start&end&buckets` — severity-stacked volume over the full match set.
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::HistogramBucket;

use crate::query_params::build_query_request;
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct HistogramParams {
    #[serde(default)]
    query: String,
    start: String,
    end: String,
    #[serde(default = "default_buckets")]
    buckets: usize,
}

fn default_buckets() -> usize {
    48
}

pub(crate) async fn histogram(
    State(state): State<AppState>,
    Query(p): Query<HistogramParams>,
) -> Response {
    let req = match build_query_request(
        &p.query,
        &p.start,
        &p.end,
        state.query.promoted_attributes(),
    ) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    match state.query.histogram(req, p.buckets).await {
        Ok(buckets) => Json(buckets.iter().map(bucket_to_json).collect::<Vec<_>>()).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

fn bucket_to_json(b: &HistogramBucket) -> Value {
    json!({
        "t": b.t.to_string(), // epoch nanos as string (matches row timestamps; dodges JS 2^53)
        "debug": b.debug,
        "info": b.info,
        "warn": b.warn,
        "error": b.error,
        "fatal": b.fatal,
        "total": b.total,
    })
}

#[cfg(test)]
mod tests {
    use tower::ServiceExt;

    #[tokio::test]
    async fn empty_server_returns_zeroed_buckets() {
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/histogram?start=0&end=100&buckets=4")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0]["total"], serde_json::json!(0));
        assert!(arr[0]["t"].is_string()); // nanos as string
    }
}
