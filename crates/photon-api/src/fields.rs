//! `GET /api/fields?start&end` — the field catalog for a window: `[{ name, kind }]` where
//! `kind ∈ fixed | promoted | attribute`. Metadata only (reads the manifest, not the data).
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_query::{FieldInfo, FieldKind};

use crate::query_params::parse_window;
use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct FieldsParams {
    start: String,
    end: String,
}

pub(crate) async fn fields(
    State(state): State<AppState>,
    Query(p): Query<FieldsParams>,
) -> Response {
    let (start, end) = match parse_window(&p.start, &p.end) {
        Ok(w) => w,
        Err(e) => return e.into_response(),
    };
    match state.query.fields(start, end) {
        Ok(fields) => Json(fields.iter().map(field_to_json).collect::<Vec<_>>()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn field_to_json(f: &FieldInfo) -> Value {
    json!({ "name": f.name, "kind": kind_str(f.kind) })
}

fn kind_str(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Fixed => "fixed",
        FieldKind::Promoted => "promoted",
        FieldKind::Attribute => "attribute",
    }
}

#[cfg(test)]
mod tests {
    use tower::ServiceExt;

    async fn get(uri: &str) -> serde_json::Value {
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
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn lists_fixed_and_promoted_fields() {
        let v = get("/api/fields?start=0&end=100").await;
        let arr = v.as_array().unwrap();
        assert!(arr
            .iter()
            .any(|f| f["name"] == "body" && f["kind"] == "fixed"));
        assert!(arr
            .iter()
            .any(|f| f["name"] == "service.name" && f["kind"] == "promoted"));
    }
}
