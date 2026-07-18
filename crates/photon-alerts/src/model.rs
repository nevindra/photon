//! Pure domain types for the alerts vertical. No I/O. All timestamps are Unix ms.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

pub type RuleId = String;
pub type ChannelId = String;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind {
    Webhook,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ChannelKind,
    pub url: String,
    #[serde(default)]
    pub secret: Option<String>,
    /// Extra request headers as a JSON object, e.g. `{"Authorization":"Bearer x"}`.
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
pub struct ChannelInput {
    pub name: String,
    #[serde(rename = "type", default = "ChannelInput::default_kind")]
    pub kind: ChannelKind,
    pub url: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
}
impl ChannelInput {
    fn default_kind() -> ChannelKind {
        ChannelKind::Webhook
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cmp {
    Gt,
    Gte,
    Lt,
    Lte,
}
impl Cmp {
    pub fn test(self, value: f64, threshold: f64) -> bool {
        match self {
            Cmp::Gt => value > threshold,
            Cmp::Gte => value >= threshold,
            Cmp::Lt => value < threshold,
            Cmp::Lte => value <= threshold,
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Cmp::Gt => ">",
            Cmp::Gte => ">=",
            Cmp::Lt => "<",
            Cmp::Lte => "<=",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

// ---- per-signal conditions ----
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricAgg {
    Avg,
    Min,
    Max,
    Sum,
    Last,
    P50,
    P90,
    P95,
    P99,
    Rate,
    Increase,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricCondition {
    pub metric_name: String,
    #[serde(default)]
    pub label_filters: BTreeMap<String, String>,
    #[serde(default)]
    pub group_by: Vec<String>,
    pub agg: MetricAgg,
    pub window_secs: i64,
    pub cmp: Cmp,
    pub threshold: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogCondition {
    pub query: String,
    #[serde(default)]
    pub group_by: Option<String>, // only "service.name" supported in v1
    pub window_secs: i64,
    pub cmp: Cmp,
    pub threshold: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    ErrorRate,
    LatencyP50,
    LatencyP90,
    LatencyP95,
    LatencyP99,
    RequestRate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceCondition {
    pub service: String,
    #[serde(default)]
    pub operation: Option<String>,
    pub kind: TraceKind,
    pub window_secs: i64,
    pub cmp: Cmp,
    pub threshold: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RumKind {
    VitalLcpP75,
    VitalInpP75,
    VitalClsP75,
    VitalFcpP75,
    VitalTtfbP75,
    ErrorCount,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RumCondition {
    pub app_id: String,
    #[serde(default)]
    pub route: Option<String>,
    pub kind: RumKind,
    pub window_secs: i64,
    pub cmp: Cmp,
    pub threshold: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "lowercase")]
pub enum Condition {
    Metrics(MetricCondition),
    Logs(LogCondition),
    Traces(TraceCondition),
    Rum(RumCondition),
}
impl Condition {
    pub fn window_secs(&self) -> i64 {
        match self {
            Condition::Metrics(c) => c.window_secs,
            Condition::Logs(c) => c.window_secs,
            Condition::Traces(c) => c.window_secs,
            Condition::Rum(c) => c.window_secs,
        }
    }
    pub fn cmp(&self) -> Cmp {
        match self {
            Condition::Metrics(c) => c.cmp,
            Condition::Logs(c) => c.cmp,
            Condition::Traces(c) => c.cmp,
            Condition::Rum(c) => c.cmp,
        }
    }
    pub fn threshold(&self) -> f64 {
        match self {
            Condition::Metrics(c) => c.threshold,
            Condition::Logs(c) => c.threshold,
            Condition::Traces(c) => c.threshold,
            Condition::Rum(c) => c.threshold,
        }
    }
    pub fn signal(&self) -> &'static str {
        match self {
            Condition::Metrics(_) => "metrics",
            Condition::Logs(_) => "logs",
            Condition::Traces(_) => "traces",
            Condition::Rum(_) => "rum",
        }
    }
    /// Human one-liner for payloads/incidents, e.g. `avg(system.cpu.utilization) > 0.90`.
    pub fn summary(&self) -> String {
        match self {
            Condition::Metrics(c) => format!(
                "{:?}({}) {} {}",
                c.agg,
                c.metric_name,
                c.cmp.symbol(),
                c.threshold
            ),
            Condition::Logs(c) => format!("count({}) {} {}", c.query, c.cmp.symbol(), c.threshold),
            Condition::Traces(c) => format!(
                "{:?}({}) {} {}",
                c.kind,
                c.service,
                c.cmp.symbol(),
                c.threshold
            ),
            Condition::Rum(c) => format!(
                "{:?}({}) {} {}",
                c.kind,
                c.app_id,
                c.cmp.symbol(),
                c.threshold
            ),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub id: RuleId,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub enabled: bool,
    pub condition: Condition,
    pub for_secs: i64,
    pub interval_secs: i64,
    pub severity: Severity,
    pub channel_ids: Vec<ChannelId>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RuleInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub condition: Condition,
    #[serde(default)]
    pub for_secs: i64,
    #[serde(default = "RuleInput::default_interval")]
    pub interval_secs: i64,
    #[serde(default = "RuleInput::default_severity")]
    pub severity: Severity,
    #[serde(default)]
    pub channel_ids: Vec<ChannelId>,
}
impl RuleInput {
    fn default_interval() -> i64 {
        60
    }
    fn default_severity() -> Severity {
        Severity::Warning
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Incident {
    pub id: i64,
    pub rule_id: RuleId,
    /// Canonical series key (`""` for an aggregate rule).
    pub series_key: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub peak_value: f64,
    pub severity: Severity,
    pub summary: String,
}

/// One evaluated series returned by a `ConditionSource`.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesSample {
    /// Ordered label pairs identifying the series (empty → aggregate).
    pub key: Vec<(String, String)>,
    pub value: f64,
}
impl SeriesSample {
    /// Stable canonical key: `k=v,k=v` sorted by key. `""` when empty.
    pub fn series_key(&self) -> String {
        let mut kv: Vec<String> = self.key.iter().map(|(k, v)| format!("{k}={v}")).collect();
        kv.sort();
        kv.join(",")
    }
    /// The label map as JSON for payloads.
    pub fn labels_json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.key
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlertPhase {
    Ok,
    Pending,
    Triggered,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeriesState {
    pub phase: AlertPhase,
    pub since: i64,
    pub last_value: f64,
}
impl SeriesState {
    pub fn ok() -> Self {
        Self {
            phase: AlertPhase::Ok,
            since: 0,
            last_value: 0.0,
        }
    }
    pub fn triggered_since(since: i64) -> Self {
        Self {
            phase: AlertPhase::Triggered,
            since,
            last_value: 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transition {
    Triggered,
    Resolved,
}

#[derive(Clone, Debug)]
pub enum SchedulerCommand {
    Upsert(Box<Rule>),
    Remove(RuleId),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cmp_tests() {
        assert!(Cmp::Gt.test(2.0, 1.0));
        assert!(!Cmp::Lte.test(2.0, 1.0));
    }
    #[test]
    fn series_key_is_sorted_and_stable() {
        let s = SeriesSample {
            key: vec![("b".into(), "2".into()), ("a".into(), "1".into())],
            value: 0.0,
        };
        assert_eq!(s.series_key(), "a=1,b=2");
        assert_eq!(
            SeriesSample {
                key: vec![],
                value: 0.0
            }
            .series_key(),
            ""
        );
    }
    #[test]
    fn condition_roundtrips_tagged() {
        let j = r#"{"signal":"metrics","metric_name":"system.cpu.utilization","agg":"avg","window_secs":300,"cmp":"gt","threshold":0.9}"#;
        let c: Condition = serde_json::from_str(j).unwrap();
        assert_eq!(c.signal(), "metrics");
        assert_eq!(c.threshold(), 0.9);
    }
}
