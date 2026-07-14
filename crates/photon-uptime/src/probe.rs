//! Active probers behind the `Prober` trait. A failed probe returns `ok:false` (data),
//! never an `Err` — only genuine bugs would panic.

use crate::model::{CheckResult, CheckType, Monitor};
use std::time::{Duration, Instant};

#[async_trait::async_trait]
pub trait Prober: Send + Sync {
    async fn probe(&self, m: &Monitor) -> CheckResult;
}

/// The real prober: reqwest (rustls) for HTTP, tokio TCP connect, surge-ping for ICMP.
pub struct NetworkProber;

impl NetworkProber {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NetworkProber {
    fn default() -> Self {
        Self::new()
    }
}

fn ms_since(start: Instant) -> u32 {
    start.elapsed().as_millis().min(u32::MAX as u128) as u32
}

fn down(err: impl Into<String>, latency_ms: u32) -> CheckResult {
    CheckResult {
        ok: false,
        latency_ms,
        status_code: None,
        error: Some(err.into()),
    }
}

/// Parse `expect_status` ("200" | "200-299" | "2xx"; default 2xx) and test a code.
fn status_matches(spec: &Option<String>, code: u16) -> bool {
    match spec.as_deref() {
        None | Some("2xx") => (200..300).contains(&code),
        Some(s) if s.contains('-') => {
            let mut it = s.split('-');
            match (
                it.next().and_then(|x| x.trim().parse::<u16>().ok()),
                it.next().and_then(|x| x.trim().parse::<u16>().ok()),
            ) {
                (Some(lo), Some(hi)) => (lo..=hi).contains(&code),
                _ => false,
            }
        }
        Some(s) => s.trim().parse::<u16>().map(|c| c == code).unwrap_or(false),
    }
}

#[async_trait::async_trait]
impl Prober for NetworkProber {
    async fn probe(&self, m: &Monitor) -> CheckResult {
        let timeout = Duration::from_secs(m.timeout_secs.max(1) as u64);
        match m.check_type {
            CheckType::Http => probe_http(m, timeout).await,
            CheckType::Tcp => probe_tcp(m, timeout).await,
            CheckType::Icmp => probe_icmp(m, timeout).await,
        }
    }
}

async fn probe_http(m: &Monitor, timeout: Duration) -> CheckResult {
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(m.ignore_tls)
        .redirect(if m.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .build()
    {
        Ok(c) => c,
        Err(e) => return down(format!("client build: {e}"), 0),
    };
    let method = reqwest::Method::from_bytes(m.http_method.as_deref().unwrap_or("GET").as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let start = Instant::now();
    let resp = match client.request(method, &m.target).send().await {
        Ok(r) => r,
        Err(e) => return down(format!("request: {e}"), ms_since(start)),
    };
    let code = resp.status().as_u16();
    let latency = ms_since(start);
    if !status_matches(&m.expect_status, code) {
        return CheckResult {
            ok: false,
            latency_ms: latency,
            status_code: Some(code),
            error: Some(format!("unexpected status {code}")),
        };
    }
    if let Some(kw) = &m.keyword {
        let body = resp.text().await.unwrap_or_default();
        if !body.contains(kw) {
            return CheckResult {
                ok: false,
                latency_ms: latency,
                status_code: Some(code),
                error: Some(format!("keyword {kw:?} not found")),
            };
        }
    }
    CheckResult {
        ok: true,
        latency_ms: latency,
        status_code: Some(code),
        error: None,
    }
}

async fn probe_tcp(m: &Monitor, timeout: Duration) -> CheckResult {
    let start = Instant::now();
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&m.target)).await {
        Ok(Ok(_stream)) => CheckResult {
            ok: true,
            latency_ms: ms_since(start),
            status_code: None,
            error: None,
        },
        Ok(Err(e)) => down(format!("connect: {e}"), ms_since(start)),
        Err(_) => down("connect timed out", ms_since(start)),
    }
}

async fn probe_icmp(m: &Monitor, timeout: Duration) -> CheckResult {
    // Resolve `target` (bare host/IP) to an IpAddr.
    let host = m.target.trim();
    let ip: std::net::IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => match tokio::net::lookup_host((host, 0))
            .await
            .ok()
            .and_then(|mut it| it.next())
        {
            Some(sa) => sa.ip(),
            None => return down(format!("dns: cannot resolve {host}"), 0),
        },
    };
    let client = match surge_ping::Client::new(&surge_ping::Config::default()) {
        Ok(c) => c,
        Err(e) => {
            return down(
                format!("icmp socket: {e} (needs CAP_NET_RAW or ping_group_range)"),
                0,
            )
        }
    };
    let mut pinger = client.pinger(ip, surge_ping::PingIdentifier(0)).await;
    pinger.timeout(timeout);
    let start = Instant::now();
    match pinger.ping(surge_ping::PingSequence(0), &[0u8; 8]).await {
        Ok((_packet, rtt)) => CheckResult {
            ok: true,
            latency_ms: rtt.as_millis().min(u32::MAX as u128) as u32,
            status_code: None,
            error: None,
        },
        Err(e) => down(format!("ping: {e}"), ms_since(start)),
    }
}

/// Test double: returns queued results in order (last repeats).
#[cfg(test)]
pub struct FakeProber {
    pub results: std::sync::Mutex<std::collections::VecDeque<CheckResult>>,
}

#[cfg(test)]
#[async_trait::async_trait]
impl Prober for FakeProber {
    async fn probe(&self, _m: &Monitor) -> CheckResult {
        let mut q = self.results.lock().unwrap();
        if q.len() > 1 {
            q.pop_front().unwrap()
        } else {
            q.front().cloned().unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CheckType, Monitor, MonitorState};

    fn mon(check_type: CheckType, target: &str) -> Monitor {
        Monitor {
            id: "t".into(),
            name: "t".into(),
            check_type,
            target: target.into(),
            interval_secs: 30,
            timeout_secs: 5,
            retries: 2,
            http_method: None,
            expect_status: None,
            keyword: None,
            ignore_tls: false,
            follow_redirects: true,
            webhook_url: None,
            enabled: true,
            last_state: MonitorState::Pending,
            last_check_at: None,
            last_latency_ms: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn tcp_probe_succeeds_against_a_live_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let p = NetworkProber::new();
        let r = p.probe(&mon(CheckType::Tcp, &addr.to_string())).await;
        assert!(r.ok, "expected up, got {:?}", r.error);
    }

    #[tokio::test]
    async fn tcp_probe_fails_against_a_dead_port() {
        // Port 1 is virtually never listening; connect refused/timeout ⇒ down.
        let p = NetworkProber::new();
        let r = p.probe(&mon(CheckType::Tcp, "127.0.0.1:1")).await;
        assert!(!r.ok);
        assert!(r.error.is_some());
    }

    #[tokio::test]
    #[ignore = "ICMP needs CAP_NET_RAW or net.ipv4.ping_group_range; run locally with --ignored"]
    async fn icmp_probe_pings_loopback() {
        let p = NetworkProber::new();
        let r = p.probe(&mon(CheckType::Icmp, "127.0.0.1")).await;
        assert!(r.ok, "loopback ping failed: {:?}", r.error);
    }
}
