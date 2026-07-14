//! `/api/infra/*` handlers — the curated host/GPU resource-monitoring surface over
//! `photon_query::infra`. All behind `require_auth`, like `/api/metrics/*`. Timestamps cross as
//! decimal-nanosecond strings (JS-safe), mirroring `metrics.rs`'s `series_json`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use photon_core::PhotonError;
use photon_query::{HostDetail, HostSummary, InfraResource, SeriesResult};

use crate::AppState;

fn err_500(e: PhotonError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

fn host_summary_json(h: &HostSummary) -> Value {
    json!({
        "host": h.host,
        "cpuUtil": h.cpu_util,
        "memUtil": h.mem_util,
        "lastSeenNs": h.last_seen_ns.to_string(),
        "hasGpu": h.has_gpu,
    })
}

fn host_detail_json(d: &HostDetail) -> Value {
    json!({
        "host": d.host,
        "os": d.os,
        "cores": d.cores,
        "totalRamBytes": d.total_ram_bytes,
        "gpus": d.gpus,
        "lastSeenNs": d.last_seen_ns.to_string(),
    })
}

fn infra_series_json(s: &SeriesResult) -> Value {
    let points: Vec<Value> = s
        .points
        .iter()
        .map(|p| json!({ "t": p.t.to_string(), "v": p.v }))
        .collect();
    json!({ "labels": s.labels, "points": points })
}

// ---------- GET /api/infra/hosts ----------

#[derive(Deserialize)]
pub(crate) struct HostsParams {
    start: i64,
    end: i64,
}

pub(crate) async fn hosts(State(st): State<AppState>, Query(p): Query<HostsParams>) -> Response {
    match st.metrics_query.infra_hosts(p.start, p.end).await {
        Ok(v) => {
            let hosts: Vec<Value> = v.iter().map(host_summary_json).collect();
            Json(json!({ "hosts": hosts })).into_response()
        }
        Err(e) => err_500(e),
    }
}

// ---------- GET /api/infra/hosts/:host ----------

pub(crate) async fn host_detail(
    State(st): State<AppState>,
    Path(host): Path<String>,
    Query(p): Query<HostsParams>,
) -> Response {
    match st
        .metrics_query
        .infra_host_detail(&host, p.start, p.end)
        .await
    {
        Ok(d) => Json(host_detail_json(&d)).into_response(),
        Err(e) => err_500(e),
    }
}

// ---------- GET /api/infra/hosts/:host/timeseries ----------

#[derive(Deserialize)]
pub(crate) struct TimeseriesParams {
    resource: String,
    start: i64,
    end: i64,
    #[serde(default)]
    buckets: Option<usize>,
}

pub(crate) async fn host_timeseries(
    State(st): State<AppState>,
    Path(host): Path<String>,
    Query(p): Query<TimeseriesParams>,
) -> Response {
    let Some(resource) = InfraResource::from_str(&p.resource) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("unknown resource `{}`", p.resource) })),
        )
            .into_response();
    };
    let buckets = p.buckets.unwrap_or(48).clamp(1, 500);
    match st
        .metrics_query
        .infra_host_series(&host, resource, p.start, p.end, buckets)
        .await
    {
        Ok(r) => {
            let series: Vec<Value> = r.series.iter().map(infra_series_json).collect();
            Json(json!({
                "resource": r.resource,
                "series": series,
            }))
            .into_response()
        }
        Err(e) => err_500(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_summary_json_stringifies_last_seen_ns() {
        let h = HostSummary {
            host: "web-1".into(),
            cpu_util: Some(0.4),
            mem_util: None,
            last_seen_ns: 1_700_000_000_000_000_000,
            has_gpu: true,
        };
        let v = host_summary_json(&h);
        assert_eq!(v["lastSeenNs"], "1700000000000000000");
        assert_eq!(v["hasGpu"], true);
        assert_eq!(v["cpuUtil"], 0.4);
        assert_eq!(v["memUtil"], Value::Null);
    }

    #[test]
    fn host_detail_json_stringifies_last_seen_ns() {
        let d = HostDetail {
            host: "web-1".into(),
            os: Some("linux".into()),
            cores: Some(8),
            total_ram_bytes: Some(34_359_738_368.0),
            gpus: vec!["NVIDIA A100".into()],
            last_seen_ns: 42,
        };
        let v = host_detail_json(&d);
        assert_eq!(v["lastSeenNs"], "42");
        assert_eq!(v["cores"], 8);
        assert_eq!(v["gpus"], serde_json::json!(["NVIDIA A100"]));
    }

    #[tokio::test]
    async fn hosts_over_empty_server_returns_empty_list() {
        use tower::ServiceExt;
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/infra/hosts?start=0&end=1")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v, json!({ "hosts": [] }));
    }

    #[tokio::test]
    async fn timeseries_unknown_resource_is_bad_request() {
        use tower::ServiceExt;
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/infra/hosts/web-1/timeseries?resource=nope&start=0&end=1")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn hosts_requires_session() {
        use tower::ServiceExt;
        let router = crate::test_router();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/infra/hosts?start=0&end=1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
