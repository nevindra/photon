//! Host CPU/RAM/disk/network sampler backed by `sysinfo` 0.33. Emits the CPU/memory/filesystem/
//! network metrics from the Global Constants (GPU is a separate `GpuSampler`, merged by the
//! sender loop in `send.rs`).
//!
//! API note (drift from the plan's sketch): in `sysinfo` 0.33, `Networks::refresh` and
//! `Disks::refresh` both take a `remove_not_listed: bool` argument (not the zero-arg signature
//! sketched in the plan) — we pass `false` (keep previously-seen devices even if transiently
//! absent from a refresh, matching `new_with_refreshed_list`'s default behavior). All other
//! names (`global_cpu_usage`, `refresh_cpu_usage`, `System::host_name`, `System::load_average`)
//! matched the plan as written.
use sysinfo::{Disks, Networks, System};

use crate::sample::{Kind, MetricSample, ResourceSample, Sampler};

pub struct SysinfoSampler {
    sys: System,
    networks: Networks,
    disks: Disks,
}

impl SysinfoSampler {
    pub fn new() -> SysinfoSampler {
        let mut sys = System::new_all();
        sys.refresh_all();
        SysinfoSampler {
            sys,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
        }
    }
}

impl Default for SysinfoSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for SysinfoSampler {
    fn sample(&mut self) -> ResourceSample {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.networks.refresh(false);
        self.disks.refresh(false);

        let g = |name, unit, value, attrs| MetricSample {
            name,
            unit,
            kind: Kind::Gauge,
            value,
            attrs,
        };
        let sum = |name, unit, value, attrs| MetricSample {
            name,
            unit,
            kind: Kind::SumMonotonic,
            value,
            attrs,
        };
        let mut metrics = Vec::new();

        // CPU aggregate + per-core
        metrics.push(g(
            "system.cpu.utilization",
            "1",
            self.sys.global_cpu_usage() as f64 / 100.0,
            vec![("cpu".into(), "total".into())],
        ));
        for (i, cpu) in self.sys.cpus().iter().enumerate() {
            metrics.push(g(
                "system.cpu.utilization",
                "1",
                cpu.cpu_usage() as f64 / 100.0,
                vec![("cpu".into(), i.to_string())],
            ));
        }
        metrics.push(g(
            "system.cpu.logical.count",
            "{cpu}",
            self.sys.cpus().len() as f64,
            vec![],
        ));
        let la = System::load_average();
        metrics.push(g("system.cpu.load_average.1m", "1", la.one, vec![]));

        // Memory
        let (total, used) = (
            self.sys.total_memory() as f64,
            self.sys.used_memory() as f64,
        );
        metrics.push(g("system.memory.limit", "By", total, vec![]));
        metrics.push(g(
            "system.memory.usage",
            "By",
            used,
            vec![("state".into(), "used".into())],
        ));
        metrics.push(g(
            "system.memory.usage",
            "By",
            (total - used).max(0.0),
            vec![("state".into(), "free".into())],
        ));
        if total > 0.0 {
            metrics.push(g("system.memory.utilization", "1", used / total, vec![]));
        }

        // Filesystems
        for d in self.disks.list() {
            let mount = d.mount_point().to_string_lossy().to_string();
            let (t, avail) = (d.total_space() as f64, d.available_space() as f64);
            metrics.push(g(
                "system.filesystem.usage",
                "By",
                (t - avail).max(0.0),
                vec![
                    ("mountpoint".into(), mount.clone()),
                    ("state".into(), "used".into()),
                ],
            ));
            if t > 0.0 {
                metrics.push(g(
                    "system.filesystem.utilization",
                    "1",
                    (t - avail).max(0.0) / t,
                    vec![("mountpoint".into(), mount)],
                ));
            }
        }

        // Network cumulative counters
        for (name, data) in self.networks.list() {
            metrics.push(sum(
                "system.network.io",
                "By",
                data.total_received() as f64,
                vec![
                    ("device".into(), name.clone()),
                    ("direction".into(), "receive".into()),
                ],
            ));
            metrics.push(sum(
                "system.network.io",
                "By",
                data.total_transmitted() as f64,
                vec![
                    ("device".into(), name.clone()),
                    ("direction".into(), "transmit".into()),
                ],
            ));
        }

        let hostname = System::host_name().unwrap_or_default();
        ResourceSample {
            host_id: stable_host_id(&hostname),
            os_type: std::env::consts::OS.to_string(),
            metrics,
        }
    }
}

/// A more stable `host.id` than the raw hostname (which can change across reimages/renames). On
/// Linux, reads the machine's stable id from `/etc/machine-id` (falling back to
/// `/var/lib/dbus/machine-id`, the older/alternate location some distros use), trimmed of
/// surrounding whitespace. On any read failure (file missing, unreadable, empty) or on a
/// non-Linux OS — where neither path exists — falls back to `hostname`, the previous behavior, so
/// `host.id` is never empty as long as the hostname itself resolved.
fn stable_host_id(hostname: &str) -> String {
    first_machine_id(&["/etc/machine-id", "/var/lib/dbus/machine-id"])
        .unwrap_or_else(|| hostname.to_string())
}

/// The trimmed content of the first path in `candidates` that exists and reads as non-empty, else
/// `None`. Factored out of `stable_host_id` so a unit test can exercise the "nothing resolves"
/// fallback with paths guaranteed not to exist, independent of whether the test host happens to
/// have a real `/etc/machine-id` (most Linux CI runners do).
fn first_machine_id(candidates: &[&str]) -> Option<String> {
    for path in candidates {
        if let Ok(contents) = std::fs::read_to_string(path) {
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Sampler;

    #[test]
    fn stable_host_id_falls_back_to_hostname_when_no_candidate_path_exists() {
        // Exercises the exact fallback `stable_host_id` performs (`first_machine_id(..)
        // .unwrap_or_else(|| hostname.to_string())`), but with paths guaranteed not to exist —
        // rather than calling `stable_host_id` itself, which hits the real `/etc/machine-id` that
        // may genuinely be present (and non-empty) on a Linux CI runner.
        let hostname = "my-host";
        let id = first_machine_id(&[
            "/definitely/does/not/exist/machine-id",
            "/also/missing/machine-id",
        ])
        .unwrap_or_else(|| hostname.to_string());
        assert_eq!(id, hostname);
    }

    #[test]
    fn sysinfo_sampler_emits_core_metrics() {
        let mut s = SysinfoSampler::new();
        let _ = s.sample(); // first read primes CPU deltas
        std::thread::sleep(std::time::Duration::from_millis(250));
        let r = s.sample();
        let names: std::collections::HashSet<_> = r.metrics.iter().map(|m| m.name).collect();
        assert!(names.contains("system.cpu.utilization"));
        assert!(names.contains("system.memory.utilization"));
        assert!(names.contains("system.memory.limit"));
    }
}
