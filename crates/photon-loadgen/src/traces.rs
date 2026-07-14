//! OTLP trace payload construction + the traces [`Payload`] impl. No I/O, so trace-tree shape is
//! unit-testable and — via a round-trip through `photon_ingest::otlp_traces_to_spans` — verified
//! to map the way the real receiver maps it.
//!
//! A "trace" here is a `Node` tree: a root `SERVER` span per entry service, `CLIENT` spans
//! fanning out to downstream `SERVER` spans in *other* services (cross-service RPC) plus DB /
//! cache / internal leaf spans, all nested in time inside their parent's window. Because
//! `service.name` is a resource attribute, spans belonging to different services within one
//! trace are grouped into separate OTLP `ResourceSpans` before encoding.

use crate::config::SpanRange;
use crate::payload::{Built, Payload};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{
    any_value::Value, AnyValue, InstrumentationScope, KeyValue,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{
    span::{Event, Link},
    ResourceSpans, ScopeSpans, Span, Status,
};
use prost::Message;
use rand::rngs::SmallRng;
use rand::Rng;
use std::collections::{BTreeMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

/// OTLP `SpanKind` values used by the generator.
const KIND_INTERNAL: i32 = 1;
const KIND_SERVER: i32 = 2;
const KIND_CLIENT: i32 = 3;

/// The traces load source: `traces_per_request` span trees per request, spread across up to
/// `services` distinct services with `spans_per_trace` spans each.
pub struct TracePayload {
    pub traces_per_request: usize,
    pub services: usize,
    pub spans_per_trace: SpanRange,
}

impl Payload for TracePayload {
    fn cost(&self) -> f64 {
        self.traces_per_request as f64
    }

    fn build(&self, rng: &mut SmallRng) -> Built {
        let (body, spans) = build_request_bytes(
            self.traces_per_request,
            self.services,
            self.spans_per_trace,
            rng,
        );
        Built {
            body,
            units: self.traces_per_request as u64,
            spans,
        }
    }
}

const HOSTS: &[&str] = &["host-a", "host-b", "host-c", "host-d"];
const ENVS: &[&str] = &["prod", "staging"];
const ROUTES: &[&str] = &[
    "GET /api/orders",
    "POST /api/checkout",
    "GET /api/users/{id}",
    "POST /api/payments",
    "GET /api/products",
    "DELETE /api/cart/{id}",
];
const RPC_METHODS: &[&str] = &[
    "OrderService/Get",
    "InventoryService/Reserve",
    "PaymentService/Charge",
    "UserService/Lookup",
    "NotifyService/Send",
];
const DB_SYSTEMS: &[&str] = &["postgresql", "mysql", "redis"];
const DB_OPS: &[&str] = &[
    "SELECT orders",
    "UPDATE inventory",
    "INSERT payments",
    "SELECT users",
];
const DB_STATEMENTS: &[&str] = &[
    "SELECT * FROM orders WHERE id = $1",
    "UPDATE inventory SET qty = qty - 1",
    "INSERT INTO payments (id, amount) VALUES ($1, $2)",
    "SELECT * FROM users WHERE id = $1",
];
const CACHE_OPS: &[&str] = &[
    "GET session",
    "GET product",
    "SETEX cart",
    "GET feature_flags",
];
const EXC_TYPES: &[&str] = &[
    "TimeoutError",
    "SqlError",
    "NullPointerException",
    "ConnectionReset",
];
const EXC_MESSAGES: &[&str] = &[
    "deadline exceeded",
    "connection reset by peer",
    "unexpected null",
    "constraint violation",
];
const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE"];

/// An intermediate, proto-agnostic span. Built with an index-based `parent` so a tree can be
/// assembled first and converted to OTLP (with random span ids) afterward.
struct Node {
    /// Index into the trace's `Vec<Node>`, or `None` for the root.
    parent: Option<usize>,
    service_idx: usize,
    host: &'static str,
    env: &'static str,
    scope_name: &'static str,
    kind: i32,
    name: String,
    /// Unix nanos.
    start: i64,
    /// Unix nanos, strictly greater than `start`.
    end: i64,
    /// `Some(message)` => status ERROR; `None` => no status (UNSET).
    error: Option<String>,
    events: Vec<Event>,
    attrs: Vec<(String, String)>,
}

/// Pick a random element out of a `&'static str` pool.
fn pick<'a>(rng: &mut SmallRng, arr: &[&'a str]) -> &'a str {
    arr[rng.gen_range(0..arr.len())]
}

/// A string-valued OTLP attribute.
fn kv(key: &str, val: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(val.to_string())),
        }),
    }
}

fn rand16(rng: &mut SmallRng) -> [u8; 16] {
    let mut b = [0u8; 16];
    rng.fill(&mut b);
    b
}

fn rand8(rng: &mut SmallRng) -> [u8; 8] {
    let mut b = [0u8; 8];
    rng.fill(&mut b);
    b
}

fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Number of `parent` hops from `idx` up to the root.
fn node_depth(nodes: &[Node], idx: usize) -> usize {
    let mut depth = 0;
    let mut cur = idx;
    while let Some(p) = nodes[cur].parent {
        depth += 1;
        cur = p;
    }
    depth
}

/// Random nanosecond bound in `[lo, hi)`, degrading to `lo` when the range is empty. Every
/// randomized nanosecond window in the generator must go through this — `rng.gen_range` panics
/// on an empty range, and tight nesting windows can easily collapse to zero width.
fn between(rng: &mut SmallRng, lo: i64, hi: i64) -> i64 {
    if hi <= lo {
        lo
    } else {
        rng.gen_range(lo..hi)
    }
}

/// Set an existing `(key, _)` pair's value in place, or push a new one.
fn set_or_push_attr(attrs: &mut Vec<(String, String)>, key: &str, val: &str) {
    if let Some(existing) = attrs.iter_mut().find(|(k, _)| k == key) {
        existing.1 = val.to_string();
    } else {
        attrs.push((key.to_string(), val.to_string()));
    }
}

/// Random HTTP-server span shape: `(route, attrs)` where `attrs` covers `http.method`,
/// `http.route`, `http.status_code`. The route is also used as the span name.
fn http_server_attrs(rng: &mut SmallRng) -> (String, Vec<(String, String)>) {
    let route = pick(rng, ROUTES).to_string();
    let attrs = vec![
        (
            "http.method".to_string(),
            pick(rng, HTTP_METHODS).to_string(),
        ),
        ("http.route".to_string(), route.clone()),
        ("http.status_code".to_string(), "200".to_string()),
    ];
    (route, attrs)
}

/// Random gRPC-client span shape: `(method, attrs)` covering `rpc.system`, `rpc.method`. The
/// method is also used as the span name.
fn rpc_client_attrs(rng: &mut SmallRng) -> (String, Vec<(String, String)>) {
    let method = pick(rng, RPC_METHODS).to_string();
    let attrs = vec![
        ("rpc.system".to_string(), "grpc".to_string()),
        ("rpc.method".to_string(), method.clone()),
    ];
    (method, attrs)
}

/// Build one trace's span tree: a root `SERVER` span plus a BFS-expanded set of children
/// (cross-service RPCs, DB/cache leaves, internal spans) until the span count reaches a target
/// sampled from `range`.
fn build_one_trace(rng: &mut SmallRng, services: usize, range: SpanRange, t0: i64) -> Vec<Node> {
    let target = if range.max <= range.min {
        range.min
    } else {
        rng.gen_range(range.min..=range.max)
    };

    let (route, root_attrs) = http_server_attrs(rng);
    let dur = rng.gen_range(50_000_000i64..800_000_000);
    let root = Node {
        parent: None,
        service_idx: rng.gen_range(0..services),
        host: pick(rng, HOSTS),
        env: pick(rng, ENVS),
        scope_name: "http.server",
        kind: KIND_SERVER,
        name: route,
        start: t0,
        end: t0 + dur,
        error: None,
        events: Vec::new(),
        attrs: root_attrs,
    };

    let mut nodes = vec![root];
    let mut frontier: VecDeque<usize> = VecDeque::new();
    frontier.push_back(0);
    let max_depth = 4;

    while nodes.len() < target {
        let parent_idx = frontier.pop_front().unwrap_or(0);
        let depth = node_depth(&nodes, parent_idx);
        if depth >= max_depth {
            // Deep nodes don't expand further; the root fallback above keeps progress going.
            continue;
        }

        let (pstart, pend, pservice, phost, penv) = {
            let p = &nodes[parent_idx];
            (p.start, p.end, p.service_idx, p.host, p.env)
        };

        let inner_lo = pstart + (pend - pstart) / 20;
        let inner_hi = pend - (pend - pstart) / 20;
        let k: i64 = rng.gen_range(1..=3);
        let inner = (inner_hi - inner_lo).max(k);
        let slot = (inner / k).max(1);

        for c in 0..k {
            if nodes.len() >= target {
                break;
            }

            let base = inner_lo + slot * c;
            let cs = base + between(rng, 0, slot / 4);
            let ce = between(rng, cs + slot / 2, cs + slot)
                .max(cs + 1)
                .min(inner_hi);

            let roll = rng.gen_range(0..100);
            if roll < 40 && depth + 1 < max_depth && services > 1 {
                // Cross-service RPC: a CLIENT span in the caller's service, whose child is a
                // SERVER span in a different service (pushed back onto the frontier so it can
                // fan out further).
                let (method, client_attrs) = rpc_client_attrs(rng);
                let client_idx = nodes.len();
                nodes.push(Node {
                    parent: Some(parent_idx),
                    service_idx: pservice,
                    host: phost,
                    env: penv,
                    scope_name: "rpc.client",
                    kind: KIND_CLIENT,
                    name: method,
                    start: cs,
                    end: ce,
                    error: None,
                    events: Vec::new(),
                    attrs: client_attrs,
                });

                let mut callee = rng.gen_range(0..services);
                if callee == pservice {
                    callee = (callee + 1) % services;
                }
                let callee_host = pick(rng, HOSTS);
                let (callee_route, callee_attrs) = http_server_attrs(rng);
                let sstart = cs + (ce - cs) / 20;
                let send = (ce - (ce - cs) / 20).max(sstart + 1);
                let server_idx = nodes.len();
                nodes.push(Node {
                    parent: Some(client_idx),
                    service_idx: callee,
                    host: callee_host,
                    env: penv,
                    scope_name: "http.server",
                    kind: KIND_SERVER,
                    name: callee_route,
                    start: sstart,
                    end: send,
                    error: None,
                    events: Vec::new(),
                    attrs: callee_attrs,
                });
                frontier.push_back(server_idx);
            } else if roll < 65 {
                // DB leaf.
                nodes.push(Node {
                    parent: Some(parent_idx),
                    service_idx: pservice,
                    host: phost,
                    env: penv,
                    scope_name: "db.client",
                    kind: KIND_CLIENT,
                    name: pick(rng, DB_OPS).to_string(),
                    start: cs,
                    end: ce,
                    error: None,
                    events: Vec::new(),
                    attrs: vec![
                        ("db.system".to_string(), pick(rng, DB_SYSTEMS).to_string()),
                        (
                            "db.statement".to_string(),
                            pick(rng, DB_STATEMENTS).to_string(),
                        ),
                    ],
                });
            } else if roll < 85 {
                // Cache leaf.
                let hit = if rng.gen_bool(0.5) { "true" } else { "false" };
                nodes.push(Node {
                    parent: Some(parent_idx),
                    service_idx: pservice,
                    host: phost,
                    env: penv,
                    scope_name: "cache.client",
                    kind: KIND_CLIENT,
                    name: pick(rng, CACHE_OPS).to_string(),
                    start: cs,
                    end: ce,
                    error: None,
                    events: Vec::new(),
                    attrs: vec![("cache.hit".to_string(), hit.to_string())],
                });
            } else {
                // Internal span; expandable (pushed back onto the frontier).
                let internal_idx = nodes.len();
                nodes.push(Node {
                    parent: Some(parent_idx),
                    service_idx: pservice,
                    host: phost,
                    env: penv,
                    scope_name: "internal",
                    kind: KIND_INTERNAL,
                    name: "compute".to_string(),
                    start: cs,
                    end: ce,
                    error: None,
                    events: Vec::new(),
                    attrs: vec![("thread.id".to_string(), rng.gen_range(1..64).to_string())],
                });
                frontier.push_back(internal_idx);
            }
        }
    }

    // Error injection: ~5% of traces get one exception on a non-root span, with the error
    // propagated up the ancestor chain (status + http.status_code) so the trace reads red
    // end-to-end.
    if nodes.len() > 1 && rng.gen_bool(0.05) {
        let victim = rng.gen_range(1..nodes.len());
        let etype = pick(rng, EXC_TYPES);
        let emsg = pick(rng, EXC_MESSAGES);
        let stacktrace = format!("at {etype} thrown during {}", nodes[victim].name);
        let event_time = nodes[victim].end as u64;

        nodes[victim].events.push(Event {
            time_unix_nano: event_time,
            name: "exception".to_string(),
            attributes: vec![
                kv("exception.type", etype),
                kv("exception.message", emsg),
                kv("exception.stacktrace", &stacktrace),
            ],
            ..Default::default()
        });

        let message = format!("{etype}: {emsg}");
        let mut cur = Some(victim);
        while let Some(idx) = cur {
            nodes[idx].error = Some(message.clone());
            set_or_push_attr(&mut nodes[idx].attrs, "http.status_code", "500");
            cur = nodes[idx].parent;
        }
    }

    nodes
}

/// Build one prost-encoded `ExportTraceServiceRequest` of `traces` traces, spread across up to
/// `services` services with span counts drawn from `range`. Returns `(encoded_bytes,
/// total_span_count)`.
pub fn build_request_bytes(
    traces: usize,
    services: usize,
    range: SpanRange,
    rng: &mut SmallRng,
) -> (Vec<u8>, u64) {
    let now = now_nanos();

    // service.name is a resource attribute, so spans of different services within (or across)
    // traces must land in separate ResourceSpans; scope_name further splits into ScopeSpans.
    let mut groups: BTreeMap<
        (usize, &'static str, &'static str),
        BTreeMap<&'static str, Vec<Span>>,
    > = BTreeMap::new();
    let mut total_spans: u64 = 0;

    for _ in 0..traces {
        let trace_id = rand16(rng).to_vec();
        // Spread trace starts over the last ~second for a nicer-looking histogram.
        let trace_t0 = now - between(rng, 0, 1_000_000_000);
        let nodes = build_one_trace(rng, services, range, trace_t0);
        let span_ids: Vec<[u8; 8]> = (0..nodes.len()).map(|_| rand8(rng)).collect();
        total_spans += nodes.len() as u64;

        for (i, node) in nodes.iter().enumerate() {
            let mut links = Vec::new();
            if node.parent.is_none() && rng.gen_bool(0.03) {
                links.push(Link {
                    trace_id: rand16(rng).to_vec(),
                    span_id: rand8(rng).to_vec(),
                    ..Default::default()
                });
            }

            let span = Span {
                trace_id: trace_id.clone(),
                span_id: span_ids[i].to_vec(),
                parent_span_id: node
                    .parent
                    .map(|p| span_ids[p].to_vec())
                    .unwrap_or_default(),
                name: node.name.clone(),
                kind: node.kind,
                start_time_unix_nano: node.start as u64,
                end_time_unix_nano: node.end as u64,
                attributes: node.attrs.iter().map(|(k, v)| kv(k, v)).collect(),
                events: node.events.clone(),
                links,
                status: node.error.as_ref().map(|m| Status {
                    code: 2,
                    message: m.clone(),
                }),
                ..Default::default()
            };

            groups
                .entry((node.service_idx, node.host, node.env))
                .or_default()
                .entry(node.scope_name)
                .or_default()
                .push(span);
        }
    }

    let mut resource_spans = Vec::with_capacity(groups.len());
    for ((service_idx, host, env), scopes) in groups {
        let mut scope_spans = Vec::with_capacity(scopes.len());
        for (scope, spans) in scopes {
            scope_spans.push(ScopeSpans {
                scope: Some(InstrumentationScope {
                    name: scope.to_string(),
                    ..Default::default()
                }),
                spans,
                ..Default::default()
            });
        }
        resource_spans.push(ResourceSpans {
            resource: Some(Resource {
                attributes: vec![
                    kv("service.name", &format!("service-{service_idx}")),
                    kv("host.name", host),
                    kv("service.version", "1.4.2"),
                    kv("deployment.environment", env),
                ],
                ..Default::default()
            }),
            scope_spans,
            ..Default::default()
        });
    }

    let req = ExportTraceServiceRequest { resource_spans };
    let mut buf = Vec::with_capacity(req.encoded_len());
    req.encode(&mut buf)
        .expect("prost encode into a Vec is infallible");
    (buf, total_spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap, HashSet};

    fn decode(bytes: &[u8]) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest::decode(bytes).expect("valid OTLP protobuf")
    }

    // Generic over the mapped record type so this module never has to name `SpanRecord`
    // directly — `photon-core` (which defines it) is only a transitive dependency here via
    // `photon-ingest`, not a dev-dependency of this crate.
    fn group_by_trace<'a, T>(
        recs: &'a [T],
        trace_id: impl Fn(&T) -> String,
    ) -> HashMap<String, Vec<&'a T>> {
        let mut by_trace: HashMap<String, Vec<&'a T>> = HashMap::new();
        for r in recs {
            by_trace.entry(trace_id(r)).or_default().push(r);
        }
        by_trace
    }

    #[test]
    fn built_request_maps_to_expected_span_count() {
        let mut rng = SmallRng::seed_from_u64(1);
        let (bytes, spans) = build_request_bytes(5, 6, SpanRange { min: 4, max: 12 }, &mut rng);
        let recs = photon_ingest::otlp_traces_to_spans(decode(&bytes));

        assert_eq!(recs.len() as u64, spans);
        assert!(
            recs.len() >= 5 * 4,
            "expected every trace to reach the min span count"
        );
    }

    #[test]
    fn each_trace_is_a_valid_tree() {
        let mut rng = SmallRng::seed_from_u64(2);
        let (bytes, _) = build_request_bytes(20, 5, SpanRange { min: 4, max: 10 }, &mut rng);
        let recs = photon_ingest::otlp_traces_to_spans(decode(&bytes));

        for group in group_by_trace(&recs, |r| r.trace_id.clone()).values() {
            let roots: Vec<_> = group
                .iter()
                .filter(|s| s.parent_span_id.is_none())
                .collect();
            assert_eq!(roots.len(), 1, "expected exactly one root span per trace");

            let ids: HashSet<&str> = group.iter().map(|s| s.span_id.as_str()).collect();
            for s in group {
                if let Some(parent) = &s.parent_span_id {
                    assert!(ids.contains(parent.as_str()), "dangling parent {parent}");
                }
            }
        }
    }

    #[test]
    fn child_spans_nested_within_parent() {
        let mut rng = SmallRng::seed_from_u64(3);
        let (bytes, _) = build_request_bytes(20, 5, SpanRange { min: 4, max: 10 }, &mut rng);
        let recs = photon_ingest::otlp_traces_to_spans(decode(&bytes));

        for group in group_by_trace(&recs, |r| r.trace_id.clone()).values() {
            let by_id: HashMap<&str, _> = group.iter().map(|s| (s.span_id.as_str(), *s)).collect();
            for s in group {
                if let Some(parent_id) = &s.parent_span_id {
                    let parent = by_id
                        .get(parent_id.as_str())
                        .expect("parent present in trace");
                    assert!(s.start_time_nanos >= parent.start_time_nanos);
                    assert!(s.end_time_nanos.unwrap() <= parent.end_time_nanos.unwrap());
                }
            }
        }
    }

    #[test]
    fn covers_service_cardinality() {
        let mut rng = SmallRng::seed_from_u64(4);
        let (bytes, _) = build_request_bytes(200, 6, SpanRange { min: 4, max: 10 }, &mut rng);
        let recs = photon_ingest::otlp_traces_to_spans(decode(&bytes));

        let seen: BTreeSet<String> = recs
            .iter()
            .map(|r| {
                r.attributes
                    .get("service.name")
                    .expect("service.name present")
                    .clone()
            })
            .collect();
        let expected: BTreeSet<String> = (0..6).map(|i| format!("service-{i}")).collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn has_both_server_and_client_and_cross_service_edges() {
        let mut rng = SmallRng::seed_from_u64(5);
        let (bytes, _) = build_request_bytes(200, 6, SpanRange { min: 4, max: 12 }, &mut rng);
        let recs = photon_ingest::otlp_traces_to_spans(decode(&bytes));

        let kinds: BTreeSet<String> = recs.iter().filter_map(|r| r.kind_text.clone()).collect();
        assert!(kinds.contains("SERVER"));
        assert!(kinds.contains("CLIENT"));

        let has_cross_service =
            group_by_trace(&recs, |r| r.trace_id.clone())
                .values()
                .any(|group| {
                    let services: BTreeSet<&str> = group
                        .iter()
                        .filter_map(|s| s.attributes.get("service.name").map(|v| v.as_str()))
                        .collect();
                    services.len() >= 2
                });
        assert!(
            has_cross_service,
            "expected at least one cross-service trace"
        );
    }

    #[test]
    fn produces_some_error_traces_with_exception_events() {
        let mut rng = SmallRng::seed_from_u64(6);
        let (bytes, _) = build_request_bytes(300, 5, SpanRange { min: 4, max: 12 }, &mut rng);
        let recs = photon_ingest::otlp_traces_to_spans(decode(&bytes));

        assert!(recs
            .iter()
            .any(|r| r.status_text.as_deref() == Some("ERROR")));
        assert!(recs.iter().any(|r| r
            .events
            .as_deref()
            .map(|e| e.contains("exception"))
            .unwrap_or(false)));
    }

    // Pull SeedableRng into scope for the seed_from_u64 calls above.
    use rand::SeedableRng;
}
