//! The resource sample model: a signal-agnostic `MetricSample` + a per-cycle `ResourceSample`
//! bundle, and the `Sampler` trait that produces one. Kept free of `sysinfo`/`nvml-wrapper` so
//! the OTLP mapping (`otlp.rs`) and the sender loop (`send.rs`) don't depend on either directly.

/// OTel metric kind emitted for a sample. Everything not a cumulative counter is a `Gauge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Gauge,
    SumMonotonic,
}

/// One data point for one metric name, with its data-point attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricSample {
    pub name: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
    pub value: f64,
    pub attrs: Vec<(String, String)>,
}

/// One sampling cycle's worth of metrics, plus the resource-level identity fields
/// (`host.id`/`os.type`) that get attached as OTLP resource attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceSample {
    pub host_id: String,
    pub os_type: String,
    pub metrics: Vec<MetricSample>,
}

/// Something that can produce a `ResourceSample` for the current instant. Implemented by
/// `SysinfoSampler` (host CPU/RAM/disk/network); GPU sampling is a separate `GpuSampler` trait
/// (`gpu.rs`) merged into the sample by the sender loop.
pub trait Sampler {
    fn sample(&mut self) -> ResourceSample;
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake;
    impl Sampler for Fake {
        fn sample(&mut self) -> ResourceSample {
            ResourceSample {
                host_id: "id-1".into(),
                os_type: "linux".into(),
                metrics: vec![MetricSample {
                    name: "system.cpu.utilization",
                    unit: "1",
                    kind: Kind::Gauge,
                    value: 0.5,
                    attrs: vec![("cpu".into(), "total".into())],
                }],
            }
        }
    }
    #[test]
    fn sampler_yields_cpu_gauge() {
        let s = Fake.sample();
        assert_eq!(s.metrics[0].name, "system.cpu.utilization");
        assert!(matches!(s.metrics[0].kind, Kind::Gauge));
    }
}
