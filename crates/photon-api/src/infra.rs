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
use photon_query::{HostDetail, HostSummary, InfraResource, ProcessSummary, SeriesResult};

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
        "diskUtil": h.disk_util,
        "diskUtilAvg": h.disk_util_avg,
        "diskGroups": h.disk_groups,
        "gpuUtil": h.gpu_util,
        "gpuUtilAvg": h.gpu_util_avg,
        "gpuGroups": h.gpu_groups,
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

fn process_summary_json(p: &ProcessSummary) -> Value {
    json!({
        "process": p.process,
        "cpuPct": p.cpu_pct,
        "rssBytes": p.rss_bytes,
        "fds": p.fds,
        "threads": p.threads,
        "restarts": p.restarts,
        "lastSeenNs": p.last_seen_ns.to_string(),
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

// ---------- GET /api/infra/hosts/:host/processes ----------

pub(crate) async fn processes(
    State(st): State<AppState>,
    Path(host): Path<String>,
    Query(p): Query<HostsParams>,
) -> Response {
    match st
        .metrics_query
        .infra_host_processes(&host, p.start, p.end)
        .await
    {
        Ok(v) => {
            let processes: Vec<Value> = v.iter().map(process_summary_json).collect();
            Json(json!({ "processes": processes })).into_response()
        }
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
            disk_util: Some(0.67),
            disk_util_avg: Some(0.355),
            disk_groups: 2,
            gpu_util: None,
            gpu_util_avg: None,
            gpu_groups: 0,
            last_seen_ns: 1_700_000_000_000_000_000,
            has_gpu: true,
        };
        let v = host_summary_json(&h);
        assert_eq!(v["lastSeenNs"], "1700000000000000000");
        assert_eq!(v["hasGpu"], true);
        assert_eq!(v["cpuUtil"], 0.4);
        assert_eq!(v["memUtil"], Value::Null);
        // The worst mountpoint and the across-mountpoint mean travel together — the UI renders
        // both, so dropping either half from the payload would strand a tile.
        assert_eq!(v["diskUtil"], 0.67);
        assert_eq!(v["diskUtilAvg"], 0.355);
        assert_eq!(v["diskGroups"], 2);
        assert_eq!(v["gpuUtil"], Value::Null);
        assert_eq!(v["gpuUtilAvg"], Value::Null);
        assert_eq!(v["gpuGroups"], 0);
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

    #[test]
    fn process_summary_json_stringifies_last_seen_ns() {
        let p = ProcessSummary {
            process: "api".into(),
            cpu_pct: Some(42.5),
            rss_bytes: Some(536_870_912.0),
            fds: Some(128.0),
            threads: Some(12.0),
            restarts: None,
            last_seen_ns: 1_700_000_000_000_000_000,
        };
        let v = process_summary_json(&p);
        assert_eq!(v["process"], "api");
        assert_eq!(v["lastSeenNs"], "1700000000000000000");
        assert_eq!(v["cpuPct"], 42.5);
        assert_eq!(v["rssBytes"], 536_870_912.0);
        assert_eq!(v["fds"], 128.0);
        assert_eq!(v["threads"], 12.0);
        assert_eq!(v["restarts"], Value::Null);
    }

    #[tokio::test]
    async fn processes_over_empty_server_returns_empty_list() {
        use tower::ServiceExt;
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/infra/hosts/web-1/processes?start=0&end=1")
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
        assert_eq!(v, json!({ "processes": [] }));
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
    async fn timeseries_accepts_the_new_resources() {
        use tower::ServiceExt;
        for resource in ["gpu_memory", "gpu_temp", "gpu_power", "load"] {
            let router = crate::test_router();
            let cookie = crate::session_cookie(&router).await;
            let resp = router
                .oneshot(
                    axum::http::Request::builder()
                        .uri(format!(
                            "/api/infra/hosts/web-1/timeseries?resource={resource}&start=0&end=1"
                        ))
                        .header(axum::http::header::COOKIE, cookie)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "resource `{resource}`");
        }
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
