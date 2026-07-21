//! Curated infrastructure query surface over the metrics store: distinct hosts + latest vitals
//! (`infra_hosts`), per-host metadata (`infra_host_detail`), and curated per-resource timeseries
//! (`infra_host_series`).
//!
//! Resource metrics (CPU/RAM/disk/network/GPU) are ordinary gauge/sum series following OTel
//! `system.*` semantic conventions, already in the metrics store — so every method reuses the
//! metrics engine's `survivors_df` + `metric_base_predicate` pruning/predicate path with **no new
//! storage engine**. `host.name` is a promoted Arrow column (`col_ref(HOST_ATTR)`), so scoping to a
//! host both prunes files (via `MetricRequest.host` → skip-index host range) and filters rows;
//! long-tail resource/data-point attributes (`os.type`, `gpu.name`, `cpu`, `mountpoint`,
//! `direction`, `gpu`) are read from the `attributes` map with `get_field`.

use std::collections::{BTreeMap, BTreeSet};

use arrow::array::{Array, Float64Array, StringArray, TimestampNanosecondArray};
use datafusion::functions::core::expr_fn::get_field;
use datafusion::functions_aggregate::expr_fn::{avg, count, max};
use datafusion::prelude::lit;

use photon_core::metric_schema;
use photon_core::query::{
    MetricFieldRef, MetricFieldResolver, MetricResolvedKind, MetricResolvedQuery,
    MetricResolvedTerm,
};
use photon_core::PhotonError;

use crate::col_ref;
use crate::metric_engine::{metric_base_predicate, MetricRequest};
use crate::MetricsQueryEngine;

// --- The exact OTel system-metric names / attributes this vertical curates (Global Constants). ---
const HOST_ATTR: &str = "host.name";
const OS_TYPE_ATTR: &str = "os.type";
const GPU_NAME_ATTR: &str = "gpu.name";

const CPU_UTIL: &str = "system.cpu.utilization";
const CPU_CORES: &str = "system.cpu.logical.count";
const MEM_UTIL: &str = "system.memory.utilization";
const MEM_LIMIT: &str = "system.memory.limit";
const FS_UTIL: &str = "system.filesystem.utilization";
const NET_IO: &str = "system.network.io";
const GPU_UTIL: &str = "system.gpu.utilization";
const GPU_MEM_UTIL: &str = "system.gpu.memory.utilization";
const GPU_TEMP: &str = "system.gpu.temperature";
const GPU_POWER: &str = "system.gpu.power";
const LOAD_1M: &str = "system.cpu.load_average.1m";

/// One host with its latest headline vitals over a window — the Infrastructure list row.
#[derive(Debug, Clone, PartialEq)]
pub struct HostSummary {
    pub host: String,
    /// Latest (window-avg) `system.cpu.utilization` in `[0,1]`, if the host reported CPU.
    pub cpu_util: Option<f64>,
    /// Latest (window-avg) `system.memory.utilization` in `[0,1]`, if reported.
    pub mem_util: Option<f64>,
    /// The WORST mountpoint's window-avg `system.filesystem.utilization` in `[0,1]` — the MAX
    /// across the host's `(host, mountpoint)` groups, if any filesystem point was reported. A
    /// plain per-host avg would dilute a real problem on one mountpoint (e.g. a full `/`) with
    /// an idle one (e.g. `/boot/efi`).
    pub disk_util: Option<f64>,
    /// The WORST GPU's window-avg `system.gpu.utilization` in `[0,1]` — the MAX across the
    /// host's `(host, gpu)` groups, if any GPU point was reported.
    pub gpu_util: Option<f64>,
    /// Newest sample timestamp seen for the host's CPU metric (epoch nanos); `0` if unknown.
    pub last_seen_ns: i64,
    /// Whether any `system.gpu.utilization` row exists for the host in the window.
    pub has_gpu: bool,
}

/// Per-host metadata for the host-detail header.
#[derive(Debug, Clone, PartialEq)]
pub struct HostDetail {
    pub host: String,
    /// Latest `os.type` map attribute for the host, if reported.
    pub os: Option<String>,
    /// Latest `system.cpu.logical.count` (rounded to an integer), if reported.
    pub cores: Option<i64>,
    /// Latest `system.memory.limit` in bytes, if reported.
    pub total_ram_bytes: Option<f64>,
    /// Distinct `gpu.name` values seen among the host's GPU points (empty if none).
    pub gpus: Vec<String>,
    /// Newest sample timestamp across the metadata metrics (epoch nanos); `0` if unknown.
    pub last_seen_ns: i64,
}

/// A curated per-resource timeseries for one host: the delegated `query_series` result, tagged
/// with the resource name.
#[derive(Debug, Clone, PartialEq)]
pub struct HostSeries {
    pub resource: String,
    pub series: Vec<crate::SeriesResult>,
}

/// The curated resource panels. `from_str` parses the API path segment; `primary` maps each
/// to its headline metric + the data-point attribute the panel breaks down by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfraResource {
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    GpuMemory,
    GpuTemp,
    GpuPower,
    Load,
}

impl InfraResource {
    #[allow(clippy::should_implement_trait)] // API wants a fallible parse, not the FromStr trait.
    pub fn from_str(s: &str) -> Option<InfraResource> {
        match s {
            "cpu" => Some(InfraResource::Cpu),
            "memory" => Some(InfraResource::Memory),
            "disk" => Some(InfraResource::Disk),
            "network" => Some(InfraResource::Network),
            "gpu" => Some(InfraResource::Gpu),
            "gpu_memory" => Some(InfraResource::GpuMemory),
            "gpu_temp" => Some(InfraResource::GpuTemp),
            "gpu_power" => Some(InfraResource::GpuPower),
            "load" => Some(InfraResource::Load),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            InfraResource::Cpu => "cpu",
            InfraResource::Memory => "memory",
            InfraResource::Disk => "disk",
            InfraResource::Network => "network",
            InfraResource::Gpu => "gpu",
            InfraResource::GpuMemory => "gpu_memory",
            InfraResource::GpuTemp => "gpu_temp",
            InfraResource::GpuPower => "gpu_power",
            InfraResource::Load => "load",
        }
    }

    /// `(metric name, group-by attribute)` for the primary panel of this resource.
    fn primary(&self) -> (&'static str, &'static str) {
        match self {
            InfraResource::Cpu => (CPU_UTIL, "cpu"),
            InfraResource::Memory => (MEM_UTIL, HOST_ATTR),
            InfraResource::Disk => (FS_UTIL, "mountpoint"),
            InfraResource::Network => (NET_IO, "direction"),
            InfraResource::Gpu => (GPU_UTIL, "gpu"),
            InfraResource::GpuMemory => (GPU_MEM_UTIL, "gpu"),
            InfraResource::GpuTemp => (GPU_TEMP, "gpu"),
            InfraResource::GpuPower => (GPU_POWER, "gpu"),
            InfraResource::Load => (LOAD_1M, HOST_ATTR),
        }
    }
}

// --- column decode helpers (downcast + null handling), mirroring `metric_catalog.rs`. ---
pub(crate) fn str_col(
    b: &arrow::record_batch::RecordBatch,
    i: usize,
) -> Result<&StringArray, PhotonError> {
    b.column(i)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PhotonError::Query(format!("infra: column {i} is not Utf8")))
}

pub(crate) fn f64_col(
    b: &arrow::record_batch::RecordBatch,
    i: usize,
) -> impl Fn(usize) -> Option<f64> + '_ {
    let a = b.column(i).as_any().downcast_ref::<Float64Array>();
    move |row| a.and_then(|a| a.is_valid(row).then(|| a.value(row)))
}

pub(crate) fn ts_col(
    b: &arrow::record_batch::RecordBatch,
    i: usize,
) -> impl Fn(usize) -> Option<i64> + '_ {
    let a = b
        .column(i)
        .as_any()
        .downcast_ref::<TimestampNanosecondArray>();
    move |row| a.and_then(|a| a.is_valid(row).then(|| a.value(row)))
}

impl MetricsQueryEngine {
    /// Distinct hosts + their latest headline vitals over `[start_ns, end_ns]`. Hosts are enumerated
    /// from the CPU-utilization metric (every agent reports it); memory utilization and GPU presence
    /// are then folded in per host. A host with no CPU points in the window does not appear.
    pub async fn infra_hosts(
        &self,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<HostSummary>, PhotonError> {
        let mut out: BTreeMap<String, HostSummary> = BTreeMap::new();

        // Distinct hosts + latest CPU utilization + last_seen from the CPU metric.
        let req = MetricRequest {
            metric: CPU_UTIL.to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: None,
        };
        if let Some(df) = self.survivors_df(&req).await? {
            let host_col = col_ref(HOST_ATTR);
            let value = col_ref(metric_schema::VALUE);
            let ts = col_ref(metric_schema::TIMESTAMP);
            let batches = df
                .filter(metric_base_predicate(&req))
                .map_err(|e| PhotonError::Query(format!("infra_hosts filter: {e}")))?
                .aggregate(
                    vec![host_col.alias("host")],
                    vec![avg(value).alias("cpu"), max(ts).alias("last_seen")],
                )
                .map_err(|e| PhotonError::Query(format!("infra_hosts aggregate: {e}")))?
                .collect()
                .await
                .map_err(|e| PhotonError::Query(format!("infra_hosts collect: {e}")))?;
            for b in &batches {
                let host = str_col(b, 0)?;
                let cpu = f64_col(b, 1);
                let last = ts_col(b, 2);
                for i in 0..b.num_rows() {
                    if host.is_valid(i) {
                        let h = host.value(i).to_string();
                        out.entry(h.clone()).or_insert(HostSummary {
                            host: h,
                            cpu_util: cpu(i),
                            mem_util: None,
                            disk_util: None,
                            gpu_util: None,
                            last_seen_ns: last(i).unwrap_or(0),
                            has_gpu: false,
                        });
                    }
                }
            }
        }

        // Latest memory utilization per host (fills mem_util).
        self.fill_latest_gauge(&mut out, MEM_UTIL, start_ns, end_ns, |h, v| {
            h.mem_util = Some(v)
        })
        .await?;
        // GPU presence: any gpu-utilization row for the host in-window.
        self.mark_gpu_presence(&mut out, start_ns, end_ns).await?;
        // Worst-mountpoint disk utilization (max of per-mountpoint window-avg).
        self.fill_worst_gauge(&mut out, FS_UTIL, "mountpoint", start_ns, end_ns, |h, v| {
            h.disk_util = Some(v)
        })
        .await?;
        // Worst-GPU utilization (max of per-gpu window-avg).
        self.fill_worst_gauge(&mut out, GPU_UTIL, "gpu", start_ns, end_ns, |h, v| {
            h.gpu_util = Some(v)
        })
        .await?;

        Ok(out.into_values().collect())
    }

    /// Set a headline gauge (window-avg per host) on the hosts already discovered by `infra_hosts`.
    /// Hosts absent from `out` (no CPU signal) are ignored — this only enriches known hosts.
    async fn fill_latest_gauge(
        &self,
        out: &mut BTreeMap<String, HostSummary>,
        metric: &str,
        start_ns: i64,
        end_ns: i64,
        set: impl Fn(&mut HostSummary, f64),
    ) -> Result<(), PhotonError> {
        let req = MetricRequest {
            metric: metric.to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: None,
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(());
        };
        let batches = df
            .filter(metric_base_predicate(&req))
            .map_err(|e| PhotonError::Query(format!("infra fill_latest_gauge filter: {e}")))?
            .aggregate(
                vec![col_ref(HOST_ATTR).alias("host")],
                vec![avg(col_ref(metric_schema::VALUE)).alias("v")],
            )
            .map_err(|e| PhotonError::Query(format!("infra fill_latest_gauge aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("infra fill_latest_gauge collect: {e}")))?;
        for b in &batches {
            let host = str_col(b, 0)?;
            let v = f64_col(b, 1);
            for i in 0..b.num_rows() {
                if host.is_valid(i) {
                    if let (Some(hs), Some(val)) = (out.get_mut(host.value(i)), v(i)) {
                        set(hs, val);
                    }
                }
            }
        }
        Ok(())
    }

    /// Set a headline gauge on the hosts already discovered by `infra_hosts`, aggregating the
    /// WORST (max) window-avg across a non-promoted data-point breakdown attribute — e.g. the
    /// worst mountpoint's disk utilization, or the worst GPU's utilization. Unlike
    /// `fill_latest_gauge` (a plain per-host avg), this groups by `(host.name,
    /// get_field(attributes, group_attr))` first, then folds the MAX per host into `out` — a
    /// plain avg would dilute a real problem on one group (a full `/` disk) with an idle one
    /// (`/boot/efi`). Hosts absent from `out` (no CPU signal) are ignored — same semantics as
    /// `fill_latest_gauge`.
    async fn fill_worst_gauge(
        &self,
        out: &mut BTreeMap<String, HostSummary>,
        metric: &str,
        group_attr: &str,
        start_ns: i64,
        end_ns: i64,
        set: impl Fn(&mut HostSummary, f64),
    ) -> Result<(), PhotonError> {
        let req = MetricRequest {
            metric: metric.to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: None,
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(());
        };
        let group = get_field(col_ref(metric_schema::ATTRIBUTES), group_attr);
        let batches = df
            .filter(metric_base_predicate(&req))
            .map_err(|e| PhotonError::Query(format!("infra fill_worst_gauge filter: {e}")))?
            .aggregate(
                vec![col_ref(HOST_ATTR).alias("host"), group.alias("group")],
                vec![avg(col_ref(metric_schema::VALUE)).alias("v")],
            )
            .map_err(|e| PhotonError::Query(format!("infra fill_worst_gauge aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("infra fill_worst_gauge collect: {e}")))?;
        let mut worst: BTreeMap<String, f64> = BTreeMap::new();
        for b in &batches {
            let host = str_col(b, 0)?;
            let v = f64_col(b, 2);
            for i in 0..b.num_rows() {
                if host.is_valid(i) {
                    if let Some(val) = v(i) {
                        worst
                            .entry(host.value(i).to_string())
                            .and_modify(|m| *m = m.max(val))
                            .or_insert(val);
                    }
                }
            }
        }
        for (host, val) in worst {
            if let Some(hs) = out.get_mut(&host) {
                set(hs, val);
            }
        }
        Ok(())
    }

    /// Mark `has_gpu` on any host that has at least one `system.gpu.utilization` row in the window.
    async fn mark_gpu_presence(
        &self,
        out: &mut BTreeMap<String, HostSummary>,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<(), PhotonError> {
        let req = MetricRequest {
            metric: GPU_UTIL.to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: None,
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(());
        };
        let batches = df
            .filter(metric_base_predicate(&req))
            .map_err(|e| PhotonError::Query(format!("infra mark_gpu_presence filter: {e}")))?
            .aggregate(
                vec![col_ref(HOST_ATTR).alias("host")],
                vec![count(lit(1i64)).alias("n")],
            )
            .map_err(|e| PhotonError::Query(format!("infra mark_gpu_presence aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("infra mark_gpu_presence collect: {e}")))?;
        for b in &batches {
            let host = str_col(b, 0)?;
            for i in 0..b.num_rows() {
                if host.is_valid(i) {
                    if let Some(hs) = out.get_mut(host.value(i)) {
                        hs.has_gpu = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// Per-host metadata: latest core count, RAM limit, OS type, and the set of GPU names, plus the
    /// newest sample timestamp across the metadata metrics. Every read is host-scoped so it prunes
    /// files by the skip-index host range and filters rows to the host. `last_seen_ns` is derived
    /// from `system.cpu.utilization` (not the core-count/mem-limit metrics used for the other
    /// fields) — the same canonical always-present metric `infra_hosts` uses for its last-seen, so
    /// host-detail and the host list agree.
    pub async fn infra_host_detail(
        &self,
        host: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<HostDetail, PhotonError> {
        let (cores_f, _) = self
            .host_latest_scalar(CPU_CORES, host, start_ns, end_ns)
            .await?;
        let (ram_f, _) = self
            .host_latest_scalar(MEM_LIMIT, host, start_ns, end_ns)
            .await?;
        let (_, last_seen_ns) = self
            .host_latest_scalar(CPU_UTIL, host, start_ns, end_ns)
            .await?;
        let gpus = self.host_gpu_names(host, start_ns, end_ns).await?;
        let os = self.host_os(host, start_ns, end_ns).await?;

        Ok(HostDetail {
            host: host.to_string(),
            os,
            cores: cores_f.map(|f| f as i64),
            total_ram_bytes: ram_f,
            gpus,
            last_seen_ns,
        })
    }

    /// Window-avg value + newest timestamp of a numeric metric for one host. `(None, 0)` when the
    /// metric has no surviving files or no matching rows for the host in the window.
    async fn host_latest_scalar(
        &self,
        metric: &str,
        host: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<(Option<f64>, i64), PhotonError> {
        let req = MetricRequest {
            metric: metric.to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: Some(host.to_string()),
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok((None, 0));
        };
        let batches = df
            .filter(metric_base_predicate(&req).and(col_ref(HOST_ATTR).eq(lit(host.to_string()))))
            .map_err(|e| PhotonError::Query(format!("infra host_latest_scalar filter: {e}")))?
            .aggregate(
                vec![],
                vec![
                    avg(col_ref(metric_schema::VALUE)).alias("v"),
                    max(col_ref(metric_schema::TIMESTAMP)).alias("last"),
                ],
            )
            .map_err(|e| PhotonError::Query(format!("infra host_latest_scalar aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("infra host_latest_scalar collect: {e}")))?;
        let Some(b) = batches.iter().find(|b| b.num_rows() > 0) else {
            return Ok((None, 0));
        };
        let value = f64_col(b, 0)(0);
        let last = ts_col(b, 1)(0).unwrap_or(0);
        Ok((value, last))
    }

    /// Latest `os.type` map attribute for a host (carried on its CPU-utilization points). `None`
    /// when no host point reports `os.type` in the window.
    async fn host_os(
        &self,
        host: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Option<String>, PhotonError> {
        let req = MetricRequest {
            metric: CPU_UTIL.to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: Some(host.to_string()),
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(None);
        };
        let os = get_field(col_ref(metric_schema::ATTRIBUTES), OS_TYPE_ATTR);
        let batches = df
            .filter(metric_base_predicate(&req).and(col_ref(HOST_ATTR).eq(lit(host.to_string()))))
            .map_err(|e| PhotonError::Query(format!("infra host_os filter: {e}")))?
            .aggregate(
                vec![os.alias("os")],
                vec![max(col_ref(metric_schema::TIMESTAMP)).alias("last")],
            )
            .map_err(|e| PhotonError::Query(format!("infra host_os aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("infra host_os collect: {e}")))?;
        // Pick the non-null os value carried by the most recent point.
        let mut best: Option<(i64, String)> = None;
        for b in &batches {
            let os_col = str_col(b, 0)?;
            let last = ts_col(b, 1);
            for i in 0..b.num_rows() {
                if os_col.is_valid(i) {
                    let ts = last(i).unwrap_or(0);
                    if best.as_ref().is_none_or(|(bt, _)| ts >= *bt) {
                        best = Some((ts, os_col.value(i).to_string()));
                    }
                }
            }
        }
        Ok(best.map(|(_, v)| v))
    }

    /// Distinct `gpu.name` values among a host's `system.gpu.utilization` points (empty if none).
    async fn host_gpu_names(
        &self,
        host: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<String>, PhotonError> {
        let req = MetricRequest {
            metric: GPU_UTIL.to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: Some(host.to_string()),
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(Vec::new());
        };
        let name = get_field(col_ref(metric_schema::ATTRIBUTES), GPU_NAME_ATTR);
        let batches = df
            .filter(metric_base_predicate(&req).and(col_ref(HOST_ATTR).eq(lit(host.to_string()))))
            .map_err(|e| PhotonError::Query(format!("infra host_gpu_names filter: {e}")))?
            .aggregate(vec![name.alias("gpu")], vec![count(lit(1i64)).alias("n")])
            .map_err(|e| PhotonError::Query(format!("infra host_gpu_names aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("infra host_gpu_names collect: {e}")))?;
        let mut names: BTreeSet<String> = BTreeSet::new();
        for b in &batches {
            let g = str_col(b, 0)?;
            for i in 0..b.num_rows() {
                if g.is_valid(i) {
                    names.insert(g.value(i).to_string());
                }
            }
        }
        Ok(names.into_iter().collect())
    }

    /// A curated per-resource timeseries for one host: delegate to the general `query_series`, one
    /// headline metric per resource, grouped by the resource's breakdown attribute. The host scope
    /// is a compiled `host.name` label filter so it flows through BOTH pruning (via
    /// `MetricRequest.host`, set in `query_series` from the filter) and the row predicate.
    pub async fn infra_host_series(
        &self,
        host: &str,
        resource: InfraResource,
        start_ns: i64,
        end_ns: i64,
        buckets: usize,
    ) -> Result<HostSeries, PhotonError> {
        let (metric, group) = resource.primary();
        // Defense-in-depth: mirrors `photon-api`'s `MAX_BUCKETS`
        // (`crates/photon-api/src/query_params.rs`); `photon-query` can't depend on `photon-api`,
        // so the value is restated here as a literal. `photon-api/src/infra.rs`'s handler already
        // clamps its own `buckets` param to 500, but this engine method is a public entry point
        // in its own right and must not trust the caller.
        let req = crate::MetricSeriesRequest {
            metric: metric.to_string(),
            agg: None,
            group_by: vec![group.to_string()],
            filter: Some(self.host_filter(host)),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            buckets: buckets.clamp(1, 3000),
        };
        let out = self.query_series(req).await?;
        Ok(HostSeries {
            resource: resource.as_str().to_string(),
            series: out.series,
        })
    }

    /// A single-term metrics filter pinning `host.name = <host>`. The field is resolved through the
    /// metrics grammar resolver so it becomes an `Attr("host.name")` (host is a promoted column) —
    /// the exact shape `metrics_host_literal` extracts for skip-index host pruning.
    fn host_filter(&self, host: &str) -> MetricResolvedQuery {
        let field = MetricFieldResolver::new(self.promoted_attributes())
            .resolve_field_name(HOST_ATTR)
            .unwrap_or(MetricFieldRef::Attr(HOST_ATTR.to_string()));
        MetricResolvedQuery {
            terms: vec![MetricResolvedTerm {
                negated: false,
                kind: MetricResolvedKind::Match {
                    field,
                    values: vec![host.to_string()],
                },
            }],
        }
    }
}

#[cfg(test)]
mod tests_fixture {
    use super::*;
    use std::sync::{Arc, Mutex};

    use arrow::array::RecordBatch;
    use object_store::local::LocalFileSystem;
    use photon_compact::MetricsCompactor;
    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::metric_schema::{metric_type, MetricSchema};
    use photon_core::segment::SegmentId;
    use photon_storage::{Replicator, Storage};
    use photon_wal::Wal;

    /// Minimal in-memory WAL handing the compactor pre-built segments, so tests control segment ids
    /// deterministically. Mirrors the `FakeWal` in `metric_catalog.rs`/`metric_engine.rs` tests.
    struct FakeWal {
        segments: Mutex<Vec<(SegmentId, Vec<RecordBatch>)>>,
    }
    #[allow(clippy::manual_async_fn)]
    impl Wal for FakeWal {
        fn append(
            &self,
            _b: RecordBatch,
        ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!() }
        }
        fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!() }
        }
        fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
            let mut ids: Vec<SegmentId> = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .map(|(id, _)| *id)
                .collect();
            ids.sort();
            Ok(ids)
        }
        fn read_segment(
            &self,
            id: SegmentId,
        ) -> impl std::future::Future<Output = Result<Vec<RecordBatch>, PhotonError>> + Send
        {
            let batches = self
                .segments
                .lock()
                .unwrap()
                .iter()
                .find(|(sid, _)| *sid == id)
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            async move { Ok(batches) }
        }
        fn remove_segment(&self, id: SegmentId) -> Result<(), PhotonError> {
            self.segments.lock().unwrap().retain(|(sid, _)| *sid != id);
            Ok(())
        }
    }

    /// A gauge point: `service.name` + `host.name` promoted, plus arbitrary long-tail attributes.
    fn mp(name: &str, host: &str, ts: i64, value: f64, attrs: &[(&str, &str)]) -> MetricPoint {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert("service.name".to_string(), "host-agent".to_string());
        attributes.insert("host.name".to_string(), host.to_string());
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::GAUGE,
            timestamp_nanos: ts,
            value: Some(value),
            attributes,
            ..Default::default()
        }
    }

    fn batch(schema: &MetricSchema, points: &[MetricPoint]) -> RecordBatch {
        let mut b = MetricBatchBuilder::new(schema);
        for p in points {
            b.append(p);
        }
        b.finish().unwrap()
    }

    /// Drive the real `MetricsCompactor` over the given `FakeWal` segments to produce genuine
    /// `data-metrics/<stem>.parquet` + `.idx` sidecars and a metrics manifest under `hot`.
    async fn compact(
        hot: &std::path::Path,
        schema: &MetricSchema,
        segments: Vec<(SegmentId, Vec<RecordBatch>)>,
    ) {
        let storage = Storage {
            hot: Arc::new(LocalFileSystem::new_with_prefix(hot).unwrap()),
            durable: None,
            hot_dir: Some(hot.to_path_buf()),
        };
        let wal = Arc::new(FakeWal {
            segments: Mutex::new(segments),
        });
        let replicator = Arc::new(Replicator::new(storage.clone()));
        let compactor = MetricsCompactor::new(wal, storage, replicator, schema.clone());
        while compactor.run_once().await.unwrap().is_some() {}
    }

    fn schema() -> MetricSchema {
        MetricSchema::new(&["service.name".to_string(), "host.name".to_string()])
    }

    /// Two hosts (`web-1`, `web-2`) each reporting CPU + memory utilization; `web-1` additionally
    /// reports a GPU and TWO filesystem mountpoints (`/` at 0.67, `/boot/efi` at 0.04 — a full
    /// disk and an idle one, so worst-mountpoint aggregation has something to prove). CPU points
    /// carry a `cpu` data-point attribute and `os.type`.
    pub(super) async fn two_hosts_cpu() -> (tempfile::TempDir, MetricsQueryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = schema();
        compact(
            &hot,
            &schema,
            vec![(
                SegmentId(0),
                vec![batch(
                    &schema,
                    &[
                        mp(
                            CPU_UTIL,
                            "web-1",
                            10,
                            0.30,
                            &[("cpu", "total"), ("os.type", "linux")],
                        ),
                        mp(
                            CPU_UTIL,
                            "web-1",
                            20,
                            0.40,
                            &[("cpu", "total"), ("os.type", "linux")],
                        ),
                        mp(MEM_UTIL, "web-1", 20, 0.55, &[("os.type", "linux")]),
                        mp(FS_UTIL, "web-1", 20, 0.67, &[("mountpoint", "/")]),
                        mp(FS_UTIL, "web-1", 22, 0.04, &[("mountpoint", "/boot/efi")]),
                        mp(
                            GPU_UTIL,
                            "web-1",
                            20,
                            0.80,
                            &[("gpu", "0"), ("gpu.name", "NVIDIA A100")],
                        ),
                        mp(
                            GPU_MEM_UTIL,
                            "web-1",
                            20,
                            0.55,
                            &[("gpu", "0"), ("gpu.name", "NVIDIA A100")],
                        ),
                        mp(
                            GPU_TEMP,
                            "web-1",
                            20,
                            61.0,
                            &[("gpu", "0"), ("gpu.name", "NVIDIA A100")],
                        ),
                        mp(
                            GPU_POWER,
                            "web-1",
                            20,
                            180.0,
                            &[("gpu", "0"), ("gpu.name", "NVIDIA A100")],
                        ),
                        mp(LOAD_1M, "web-1", 20, 1.25, &[("os.type", "linux")]),
                        mp(
                            CPU_UTIL,
                            "web-2",
                            15,
                            0.10,
                            &[("cpu", "total"), ("os.type", "linux")],
                        ),
                        mp(
                            CPU_UTIL,
                            "web-2",
                            25,
                            0.20,
                            &[("cpu", "total"), ("os.type", "linux")],
                        ),
                        mp(MEM_UTIL, "web-2", 25, 0.35, &[("os.type", "linux")]),
                    ],
                )],
            )],
        )
        .await;
        let engine = MetricsQueryEngine::new(hot, schema).unwrap();
        (dir, engine)
    }

    /// A single host (`web-1`) reporting a logical-core count of 8 plus a memory limit, so the
    /// detail view can report cores + last_seen.
    pub(super) async fn host_with_core_count() -> (tempfile::TempDir, MetricsQueryEngine) {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        let schema = schema();
        compact(
            &hot,
            &schema,
            vec![(
                SegmentId(0),
                vec![batch(
                    &schema,
                    &[
                        mp(CPU_CORES, "web-1", 10, 8.0, &[("os.type", "linux")]),
                        mp(MEM_LIMIT, "web-1", 10, 34_359_738_368.0, &[]),
                        mp(
                            CPU_UTIL,
                            "web-1",
                            10,
                            0.5,
                            &[("cpu", "total"), ("os.type", "linux")],
                        ),
                    ],
                )],
            )],
        )
        .await;
        let engine = MetricsQueryEngine::new(hot, schema).unwrap();
        (dir, engine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn infra_hosts_lists_distinct_hosts_with_latest_cpu() {
        let (_dir, engine) = super::tests_fixture::two_hosts_cpu().await;
        let mut hosts = engine.infra_hosts(0, i64::MAX).await.unwrap();
        hosts.sort_by(|a, b| a.host.cmp(&b.host));
        assert_eq!(
            hosts.iter().map(|h| h.host.clone()).collect::<Vec<_>>(),
            vec!["web-1".to_string(), "web-2".to_string()]
        );
        assert!(hosts[0].cpu_util.is_some());
        assert!(hosts[0].has_gpu, "web-1 reported a GPU");
        assert!(!hosts[1].has_gpu, "web-2 has no GPU");
    }

    #[tokio::test]
    async fn infra_hosts_reports_worst_disk_and_gpu_util() {
        let (_dir, engine) = super::tests_fixture::two_hosts_cpu().await;
        let mut hosts = engine.infra_hosts(0, i64::MAX).await.unwrap();
        hosts.sort_by(|a, b| a.host.cmp(&b.host));
        let web1 = &hosts[0];
        let web2 = &hosts[1];

        // The WORST mountpoint (max), NOT the ~0.355 avg of 0.67 and 0.04 — this is the point of
        // the test: a plain per-host avg would hide a nearly-full `/` behind an idle `/boot/efi`.
        let disk = web1.disk_util.expect("web-1 reported filesystem points");
        assert!(
            (disk - 0.67).abs() < 1e-9,
            "expected the max mountpoint (0.67), got {disk} (a plain avg would be ~0.355)"
        );
        let gpu = web1.gpu_util.expect("web-1 reported a GPU point");
        assert!((gpu - 0.80).abs() < 1e-9, "expected 0.80, got {gpu}");

        assert_eq!(web2.disk_util, None, "web-2 has no filesystem points");
        assert_eq!(web2.gpu_util, None, "web-2 has no GPU points");
    }

    #[tokio::test]
    async fn infra_host_detail_reports_cores_and_last_seen() {
        let (_dir, engine) = super::tests_fixture::host_with_core_count().await;
        let d = engine
            .infra_host_detail("web-1", 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(d.host, "web-1");
        assert_eq!(d.cores, Some(8));
        assert!(d.last_seen_ns > 0);
    }

    #[tokio::test]
    async fn infra_host_series_cpu_returns_bucketed_series_for_host() {
        let (_dir, engine) = super::tests_fixture::two_hosts_cpu().await;
        let r = engine
            .infra_host_series("web-1", InfraResource::Cpu, 0, i64::MAX, 12)
            .await
            .unwrap();
        assert_eq!(r.resource, "cpu");
        assert!(!r.series.is_empty());
        // series are scoped to web-1 only
        for s in &r.series {
            if let Some(h) = s.labels.get("host.name") {
                assert_eq!(h, "web-1");
            }
        }
    }

    #[tokio::test]
    async fn infra_host_series_clamps_a_dos_sized_bucket_count() {
        // Defense-in-depth for `infra_host_series` itself — `photon-api/src/infra.rs`'s handler
        // already clamps its own `buckets` param to 500, but a direct engine caller must not be
        // able to drive a multi-million-point series via `buckets`.
        let (_dir, engine) = super::tests_fixture::two_hosts_cpu().await;
        let r = engine
            .infra_host_series("web-1", InfraResource::Cpu, 0, i64::MAX, 10_000_000)
            .await
            .unwrap();
        for s in &r.series {
            assert!(
                s.points.len() <= 3000,
                "buckets must be clamped to MAX_BUCKETS, got {}",
                s.points.len()
            );
        }
    }

    #[test]
    fn infra_resource_parses_the_new_resources() {
        assert_eq!(
            InfraResource::from_str("gpu_memory"),
            Some(InfraResource::GpuMemory)
        );
        assert_eq!(
            InfraResource::from_str("gpu_temp"),
            Some(InfraResource::GpuTemp)
        );
        assert_eq!(
            InfraResource::from_str("gpu_power"),
            Some(InfraResource::GpuPower)
        );
        assert_eq!(InfraResource::from_str("load"), Some(InfraResource::Load));
        assert_eq!(InfraResource::from_str("nope"), None);
    }

    #[tokio::test]
    async fn infra_host_series_serves_the_new_gpu_and_load_resources() {
        let (_dir, engine) = super::tests_fixture::two_hosts_cpu().await;
        for (resource, name) in [
            (InfraResource::GpuMemory, "gpu_memory"),
            (InfraResource::GpuTemp, "gpu_temp"),
            (InfraResource::GpuPower, "gpu_power"),
            (InfraResource::Load, "load"),
        ] {
            let r = engine
                .infra_host_series("web-1", resource, 0, i64::MAX, 12)
                .await
                .unwrap();
            assert_eq!(r.resource, name);
            assert!(!r.series.is_empty(), "{name} series must not be empty");
        }
    }
}
