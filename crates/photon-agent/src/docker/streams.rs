//! Agent-initiated streams (section 7.4): container logs, live stats, and an exec shell, each
//! uploaded to `POST /api/agent/stream/{session}` as newline-delimited `StreamChunk`s over one
//! chunked request that stays open while the source flows. Same direction, same token, same
//! firewall rule as the sync. The server closes the request when the UI stops reading; the
//! agent then tears the source down.
//!
//! Exec input rides the other way as a long-poll: `GET …/{session}/input?wait=20` returns the
//! lines typed since the last call, or `closed: true` when the server ended the session.
use std::time::Duration;

use bollard::container::LogOutput;
use bollard::exec::{StartExecOptions, StartExecResults};
use bollard::query_parameters::{LogsOptionsBuilder, StatsOptionsBuilder};
use bollard::Docker;
use futures::StreamExt;
use photon_deploy::model::AgentCapability;
use photon_deploy::proto::{ExecInput, StreamChunk, StreamKind, StreamSpec};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use super::actions::exec_options;
use super::inventory::read_stats;
use super::stats::RateTracker;
use super::Caps;
use crate::clock::{now_ms, parse_rfc3339_ms, Skew};

/// Everything a stream task needs to reach the server.
#[derive(Clone)]
pub struct Uplink {
    pub client: reqwest::Client,
    pub server: String,
    pub token: String,
}

impl Uplink {
    fn url(&self, path: &str) -> String {
        format!("{}/api/agent/{}", self.server.trim_end_matches('/'), path)
    }
}

/// Turn a chunk into one NDJSON line.
fn line(chunk: &StreamChunk) -> bytes::Bytes {
    let mut s = serde_json::to_string(chunk).unwrap_or_default();
    s.push('\n');
    bytes::Bytes::from(s)
}

/// Split a Docker log line with `timestamps=true` into (ts_ms, text).
pub fn split_log_line(raw: &str, fallback_ms: i64) -> (i64, String) {
    let raw = raw.trim_end_matches(['\n', '\r']);
    if let Some((ts, rest)) = raw.split_once(' ') {
        if let Some(ms) = parse_rfc3339_ms(ts) {
            return (ms, rest.to_string());
        }
    }
    (fallback_ms, raw.to_string())
}

/// Start one stream: returns the task handle; aborting it stops both the source and the upload.
pub fn spawn(
    docker: Docker,
    caps: Caps,
    uplink: Uplink,
    spec: StreamSpec,
    skew: std::sync::Arc<Skew>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (tx, rx) = mpsc::channel::<bytes::Bytes>(256);
        let session = spec.session.clone();
        // The upload: one chunked POST fed from the channel. When the server closes the
        // session it answers, the request completes, and the source loop below notices the
        // channel is closed.
        let up = uplink.clone();
        let upload = tokio::spawn(async move {
            let stream =
                tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok::<_, std::io::Error>);
            let res = up
                .client
                .post(up.url(&format!("stream/{session}")))
                .bearer_auth(&up.token)
                .header(reqwest::header::CONTENT_TYPE, "application/x-ndjson")
                .body(reqwest::Body::wrap_stream(stream))
                .send()
                .await;
            match res {
                Ok(r)
                    if r.status().is_success()
                        || r.status().as_u16() == 404
                        || r.status().as_u16() == 410 => {}
                Ok(r) => eprintln!(
                    "photon-agent: stream upload for {session} answered {}",
                    r.status()
                ),
                Err(e) => eprintln!("photon-agent: stream upload for {session} failed: {e}"),
            }
        });

        let result: Result<(), String> = match spec.kind {
            StreamKind::Logs => match caps.require(AgentCapability::Read) {
                Ok(()) => run_logs(&docker, &spec, &tx, &skew).await,
                Err(e) => Err(e),
            },
            StreamKind::Stats => match caps.require(AgentCapability::Read) {
                Ok(()) => run_stats(&docker, &spec, &tx, &skew).await,
                Err(e) => Err(e),
            },
            StreamKind::Exec => match caps.require(AgentCapability::Exec) {
                Ok(()) => run_exec(&docker, &uplink, &spec, &tx).await,
                Err(e) => Err(e),
            },
        };
        let end = match result {
            Ok(()) => StreamChunk::End {
                exit_code: None,
                error: None,
            },
            Err(e) => StreamChunk::End {
                exit_code: None,
                error: Some(e),
            },
        };
        let _ = tx.send(line(&end)).await;
        drop(tx);
        // Give the upload a moment to flush the End line, then stop waiting on it.
        let _ = tokio::time::timeout(Duration::from_secs(5), upload).await;
    })
}

async fn run_logs(
    docker: &Docker,
    spec: &StreamSpec,
    tx: &mpsc::Sender<bytes::Bytes>,
    skew: &Skew,
) -> Result<(), String> {
    let mut b = LogsOptionsBuilder::new()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .timestamps(true);
    if let Some(n) = spec.tail {
        b = b.tail(&n.to_string());
    } else {
        b = b.tail("200");
    }
    if let Some(since) = spec.since_ms {
        b = b.since((since / 1000) as i32);
    }
    let mut stream = docker.logs(&spec.container_id, Some(b.build()));
    while let Some(item) = stream.next().await {
        let out = item.map_err(|e| e.to_string())?;
        let (stream_name, bytes) = match out {
            LogOutput::StdErr { message } => ("stderr", message),
            LogOutput::StdOut { message } => ("stdout", message),
            LogOutput::Console { message } => ("stdout", message),
            LogOutput::StdIn { message } => ("stdin", message),
        };
        let text = String::from_utf8_lossy(&bytes);
        for raw in text.split_inclusive('\n') {
            if raw.trim().is_empty() {
                continue;
            }
            let (ts, t) = split_log_line(raw, skew.now());
            let chunk = StreamChunk::Log {
                ts_ms: ts,
                stream: stream_name.to_string(),
                text: t,
            };
            if tx.send(line(&chunk)).await.is_err() {
                return Ok(()); // server closed the session
            }
        }
    }
    Ok(())
}

async fn run_stats(
    docker: &Docker,
    spec: &StreamSpec,
    tx: &mpsc::Sender<bytes::Bytes>,
    skew: &Skew,
) -> Result<(), String> {
    let mut stream = docker.stats(
        &spec.container_id,
        Some(StatsOptionsBuilder::new().stream(true).build()),
    );
    let mut tracker = RateTracker::default();
    let mut n: u64 = 0;
    while let Some(item) = stream.next().await {
        let s = item.map_err(|e| e.to_string())?;
        n += 1;
        let ts = skew.now();
        let (cpu, counters, mem, mem_limit) = read_stats(&s, now_ms());
        let rates = tracker.update(&spec.container_id, cpu, counters);
        // Docker emits one sample a second; every second one is plenty for a chart.
        if n % 2 == 1 {
            continue;
        }
        let chunk = StreamChunk::Stat {
            ts_ms: ts,
            cpu_pct: rates.cpu_pct,
            mem_bytes: mem,
            mem_limit_bytes: mem_limit,
            net_rx_bps: rates.net_rx_bps,
            net_tx_bps: rates.net_tx_bps,
            block_read_bps: rates.block_read_bps,
            block_write_bps: rates.block_write_bps,
        };
        if tx.send(line(&chunk)).await.is_err() {
            return Ok(());
        }
    }
    Ok(())
}

async fn run_exec(
    docker: &Docker,
    uplink: &Uplink,
    spec: &StreamSpec,
    tx: &mpsc::Sender<bytes::Bytes>,
) -> Result<(), String> {
    let created = docker
        .create_exec(&spec.container_id, exec_options(spec.shell.as_deref()))
        .await
        .map_err(|e| e.to_string())?;
    let exec_id = created.id.clone();
    let started = docker
        .start_exec(
            &exec_id,
            Some(StartExecOptions {
                detach: false,
                tty: false,
                output_capacity: None,
            }),
        )
        .await
        .map_err(|e| e.to_string())?;
    let (mut output, mut input) = match started {
        StartExecResults::Attached { output, input } => (output, input),
        StartExecResults::Detached => return Err("exec started detached".into()),
    };
    // Input: long-poll the server for typed lines until it says closed.
    let (in_tx, mut in_rx) = mpsc::channel::<Option<String>>(64);
    let poller = {
        let up = uplink.clone();
        let session = spec.session.clone();
        tokio::spawn(async move {
            loop {
                let res = up
                    .client
                    .get(up.url(&format!("stream/{session}/input")))
                    .query(&[("wait", "20")])
                    .bearer_auth(&up.token)
                    .timeout(Duration::from_secs(40))
                    .send()
                    .await;
                match res {
                    Ok(r) if r.status().is_success() => {
                        let body: ExecInput = r.json().await.unwrap_or_default();
                        for l in body.lines {
                            if in_tx.send(Some(l)).await.is_err() {
                                return;
                            }
                        }
                        if body.closed {
                            let _ = in_tx.send(None).await;
                            return;
                        }
                    }
                    Ok(r) if r.status().as_u16() == 404 || r.status().as_u16() == 410 => {
                        let _ = in_tx.send(None).await;
                        return;
                    }
                    Ok(_) | Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
                }
            }
        })
    };
    let result = loop {
        tokio::select! {
            out = output.next() => match out {
                Some(Ok(o)) => {
                    let text = String::from_utf8_lossy(&o.into_bytes()).to_string();
                    if tx.send(line(&StreamChunk::Exec { text })).await.is_err() { break Ok(()); }
                }
                Some(Err(e)) => break Err(e.to_string()),
                None => break Ok(()),
            },
            inp = in_rx.recv() => match inp {
                Some(Some(l)) => {
                    let mut l = l;
                    l.push('\n');
                    if input.write_all(l.as_bytes()).await.is_err() { break Ok(()); }
                    let _ = input.flush().await;
                }
                Some(None) | None => {
                    let _ = input.shutdown().await;
                    break Ok(());
                }
            },
        }
    };
    poller.abort();
    let exit = docker
        .inspect_exec(&exec_id)
        .await
        .ok()
        .and_then(|i| i.exit_code);
    let _ = tx
        .send(line(&StreamChunk::End {
            exit_code: exit,
            error: None,
        }))
        .await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_timestamp_prefix_is_split_off() {
        let (ts, text) = split_log_line("2025-09-07T10:11:12.123456789Z hello world\n", 5);
        assert!(ts > 1_700_000_000_000, "{ts}");
        assert_eq!(text, "hello world");
        let (ts, text) = split_log_line("no timestamp here", 5);
        assert_eq!((ts, text.as_str()), (5, "no timestamp here"));
    }

    #[test]
    fn chunks_are_ndjson_lines() {
        let l = line(&StreamChunk::Exec { text: "$ ".into() });
        assert!(l.ends_with(b"\n"));
        assert!(std::str::from_utf8(&l)
            .unwrap()
            .starts_with("{\"t\":\"exec\""));
    }
}
