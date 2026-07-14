//! `GET /api/facet?field&query&start&end&limit` — top field values + counts over the match set.
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::query_params::{build_query_request, clamp_limit};
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct FacetParams {
    field: String,
    #[serde(default)]
    query: String,
    start: String,
    end: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    50
}

pub(crate) async fn facet(State(state): State<AppState>, Query(p): Query<FacetParams>) -> Response {
    let req = match build_query_request(
        &p.query,
        &p.start,
        &p.end,
        state.query.promoted_attributes(),
    ) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    match state.query.facet(&p.field, req, clamp_limit(p.limit)).await {
        Ok(r) => Json(json!({
            "values": r.values.iter()
                .map(|v| json!({ "value": v.value, "count": v.count }))
                .collect::<Vec<_>>(),
            "capped": r.capped,
        }))
        .into_response(),
        // A bad facet field (e.g. `level`, `body`) surfaces here as a query error → 400.
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use tower::ServiceExt;

    async fn get(uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
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
    async fn empty_server_returns_empty_facet() {
        let (status, v) = get("/api/facet?field=service.name&start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["values"], serde_json::json!([]));
        assert_eq!(v["capped"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn faceting_on_level_is_a_bad_request() {
        let (status, _v) = get("/api/facet?field=level&start=0&end=100").await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn facet_clamps_a_dos_sized_limit() {
        let (status, v) =
            get("/api/facet?field=service.name&start=0&end=100&limit=999999999").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(v["values"], serde_json::json!([]));
        assert_eq!(v["capped"], serde_json::json!(false));
    }
}
