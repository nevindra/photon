//! NVIDIA GPU sampler via NVML, behind the `gpu` feature (default-on; opt out at runtime with
//! `--no-gpu`/`PHOTON_AGENT_NO_GPU`, or at compile time with `--no-default-features`). `NoGpu`
//! is always compiled and is the fallback whenever GPU sampling is disabled or NVML fails to
//! initialize (no NVIDIA driver present, e.g. on macOS dev hosts) — the agent must never fail to
//! start just because there's no GPU.
#[cfg(feature = "gpu")]
use crate::sample::Kind;
use crate::sample::MetricSample;

pub trait GpuSampler {
    fn sample(&mut self) -> Vec<MetricSample>;
}

pub struct NoGpu;
impl GpuSampler for NoGpu {
    fn sample(&mut self) -> Vec<MetricSample> {
        Vec::new()
    }
}

/// Returns an `NvmlGpu` when GPU sampling is enabled, built with the `gpu` feature, and NVML
/// initializes successfully; otherwise falls back to `NoGpu` (logging once why).
pub fn init(enabled: bool) -> Box<dyn GpuSampler + Send> {
    if !enabled {
        return Box::new(NoGpu);
    }
    #[cfg(feature = "gpu")]
    {
        match nvml_wrapper::Nvml::init() {
            Ok(nvml) => return Box::new(NvmlGpu { nvml }),
            Err(e) => eprintln!("photon-agent: NVML init failed, GPU metrics disabled: {e}"),
        }
    }
    Box::new(NoGpu)
}

#[cfg(feature = "gpu")]
pub struct NvmlGpu {
    nvml: nvml_wrapper::Nvml,
}

#[cfg(feature = "gpu")]
impl GpuSampler for NvmlGpu {
    fn sample(&mut self) -> Vec<MetricSample> {
        let mut out = Vec::new();
        let count = match self.nvml.device_count() {
            Ok(c) => c,
            Err(_) => return out,
        };
        for i in 0..count {
            let Ok(dev) = self.nvml.device_by_index(i) else {
                continue;
            };
            let name = dev.name().unwrap_or_else(|_| format!("gpu-{i}"));
            let attrs = || {
                vec![
                    ("gpu".to_string(), i.to_string()),
                    ("gpu.name".to_string(), name.clone()),
                ]
            };
            if let Ok(u) = dev.utilization_rates() {
                out.push(MetricSample {
                    name: "system.gpu.utilization",
                    unit: "1",
                    kind: Kind::Gauge,
                    value: u.gpu as f64 / 100.0,
                    attrs: attrs(),
                });
            }
            if let Ok(m) = dev.memory_info() {
                out.push(MetricSample {
                    name: "system.gpu.memory.usage",
                    unit: "By",
                    kind: Kind::Gauge,
                    value: m.used as f64,
                    attrs: attrs(),
                });
                if m.total > 0 {
                    out.push(MetricSample {
                        name: "system.gpu.memory.utilization",
                        unit: "1",
                        kind: Kind::Gauge,
                        value: m.used as f64 / m.total as f64,
                        attrs: attrs(),
                    });
                }
            }
            if let Ok(t) =
                dev.temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
            {
                out.push(MetricSample {
                    name: "system.gpu.temperature",
                    unit: "Cel",
                    kind: Kind::Gauge,
                    value: t as f64,
                    attrs: attrs(),
                });
            }
            if let Ok(p) = dev.power_usage() {
                out.push(MetricSample {
                    name: "system.gpu.power",
                    unit: "W",
                    kind: Kind::Gauge,
                    value: p as f64 / 1000.0,
                    attrs: attrs(),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_gpu_yields_no_metrics() {
        let mut g = NoGpu;
        assert!(g.sample().is_empty());
    }
    #[test]
    fn init_disabled_returns_no_gpu() {
        let mut g = init(false);
        assert!(g.sample().is_empty());
    }
}
