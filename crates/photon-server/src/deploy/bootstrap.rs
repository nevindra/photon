//! The SSH bootstrap installer (R1.9): the operator types an address plus SSH credentials and
//! this Photon installs and enrols the agent itself — "copy-pastes nothing and never sees a
//! token". `photon-api` owns the job record (`DeployHub::put_job` / `update_job`) and the
//! `Bootstrapper` seam; this file is the photon-server side that actually dials the host.
//!
//! One job = N addresses, run concurrently (at most `MAX_CONCURRENT` at a time), each walking the
//! six steps of `BootstrapStepKey::ALL` and reporting into its `BootstrapHostProgress`:
//!
//! 1. **Connect** — TCP + SSH as the given user, password or private key. Any host key is
//!    accepted (this is a first contact from the operator's own control plane; the key's
//!    fingerprint is recorded in the step detail so it can be checked after the fact).
//! 2. **Detect** — `DETECT_SCRIPT` over `sh -c`: architecture, OS, hostname, uid, and whether
//!    docker / systemd / passwordless sudo are there. Root or `sudo -n` is required; a missing
//!    docker is a warning, not a failure (the agent still reports host metrics).
//! 3. **Push** — the agent binary for that architecture from `DeployApi::agent_binary_path`,
//!    streamed to `cat > /tmp/photon-agent.<random>` over an exec channel and verified with
//!    `sha256sum` against the local digest.
//! 4. **Configure** — enrol the host (or rotate the token of one already enrolled), then run
//!    `photon-agent install …` remotely with the freshly minted token. The token travels only
//!    inside that exec command: it never lands in a log line, an audit row or a step detail,
//!    and any remote output surfaced to the operator passes through `redact`.
//! 5. **Start** — `systemctl is-active photon-agent`, retried for `START_TIMEOUT`. A host
//!    without systemd is Done-with-a-note: `install` printed the unit for the operator.
//! 6. **Sync** — wait `SYNC_TIMEOUT` for the store to record the agent's first sync.
//!
//! **Credentials are never stored.** `SshBootstrapInput::remember_credentials` is echoed into
//! `BootstrapJob::credentials_kept` by the API and otherwise ignored here: there is no
//! credential store yet, so the flag documents an intent the server does not honour.

use photon_api::deploy::agents::{normalize_arch, DEFAULT_POLL_SECS, MIN_POLL_SECS};
use photon_api::deploy::{now_ms, BootstrapCtx, Bootstrapper, DeployHub};
use photon_deploy::ids::{mint_token, new_id, sha256_hex};
use photon_deploy::model::*;
use russh::client::{self, Handle, Handler, KeyboardInteractiveAuthResponse};
use russh::keys::{decode_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Addresses of one job bootstrapped at the same time.
pub const MAX_CONCURRENT: usize = 4;
/// TCP connect + SSH handshake + authentication, together.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Any single short command (detect, chmod, sha256sum, systemctl).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Streaming the agent binary; a 15 MB static binary over a slow site link.
const PUSH_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// `photon-agent install` (copies the binary, writes the unit, `systemctl enable --now`).
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
/// How long `systemctl is-active` is retried after install.
pub const START_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the Sync step waits for the agent's first sync to reach the store.
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(90);
const SYNC_POLL: Duration = Duration::from_secs(2);

/// The identity the enrolment and audit rows carry for a bootstrap run: the operator who
/// started the job is already on the `agent.bootstrap` audit row the API wrote.
const ACTOR: &str = "bootstrap";

/// What Detect runs, through `sh -c`, so it works under any login shell. Every fact is a
/// `key=value` line so `parse_detect` never has to guess which line was which.
pub const DETECT_SCRIPT: &str = r#"echo "arch=$(uname -m 2>/dev/null)"; echo "os=$(uname -s 2>/dev/null)"; echo "hostname=$(hostname 2>/dev/null || cat /etc/hostname 2>/dev/null || uname -n)"; echo "uid=$(id -u)"; echo "pretty=$(. /etc/os-release 2>/dev/null && printf %s "$PRETTY_NAME")"; for t in docker systemctl sudo; do if command -v $t >/dev/null 2>&1; then echo "$t=yes"; else echo "$t=no"; fi; done; if [ -d /run/systemd/system ]; then echo systemd=yes; else echo systemd=no; fi; if [ "$(id -u)" = 0 ]; then echo sudo_nopass=yes; elif sudo -n true >/dev/null 2>&1; then echo sudo_nopass=yes; else echo sudo_nopass=no; fi"#;

/// The photon-server side of `Bootstrapper`: a real SSH client (russh) driven from a tokio task
/// per address. Stateless — everything a run needs arrives in `BootstrapCtx`.
pub struct SshBootstrapper;

impl Bootstrapper for SshBootstrapper {
    fn start(&self, job_id: String, input: SshBootstrapInput, ctx: BootstrapCtx) {
        let input = Arc::new(input);
        let ctx = Arc::new(ctx);
        tokio::spawn(async move {
            let sem = Arc::new(Semaphore::new(MAX_CONCURRENT));
            let mut tasks = Vec::with_capacity(input.addresses.len());
            for (idx, address) in input.addresses.iter().cloned().enumerate() {
                let Ok(permit) = sem.clone().acquire_owned().await else {
                    break;
                };
                let progress = Progress {
                    hub: ctx.api.hub.clone(),
                    job_id: job_id.clone(),
                    idx,
                };
                let (input, ctx) = (input.clone(), ctx.clone());
                tasks.push(tokio::spawn(async move {
                    let _permit = permit;
                    run_host(&progress, &address, &input, &ctx).await;
                }));
            }
            for t in tasks {
                let _ = t.await;
            }
            ctx.api.hub.update_job(&job_id, |job| {
                for h in &mut job.hosts {
                    // A panicked task is the only way to get here still Running.
                    if h.status == JobStatus::Running {
                        h.status = JobStatus::Failed;
                        h.error = Some("bootstrap task ended without a result".into());
                    }
                }
                job.status = if job.hosts.iter().all(|h| h.status == JobStatus::Done) {
                    JobStatus::Done
                } else {
                    JobStatus::Failed
                };
            });
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------------------------

/// One host's slot in the job, addressed by index (the API builds `job.hosts` from the same
/// address list, in order).
struct Progress {
    hub: Arc<DeployHub>,
    job_id: String,
    idx: usize,
}

type StepErr = (BootstrapStepKey, String);

impl Progress {
    fn update(&self, f: impl FnOnce(&mut BootstrapHostProgress)) {
        self.hub.update_job(&self.job_id, |job| {
            if let Some(h) = job.hosts.get_mut(self.idx) {
                f(h);
            }
        });
    }

    fn step(&self, key: BootstrapStepKey, status: StepStatus, detail: Option<String>) {
        self.update(|h| {
            if let Some(s) = h.steps.iter_mut().find(|s| s.key == key) {
                s.status = status;
                if detail.is_some() {
                    s.detail = detail;
                }
            }
        });
    }

    fn running(&self, key: BootstrapStepKey) {
        self.step(key, StepStatus::Running, None);
    }

    fn done(&self, key: BootstrapStepKey, detail: impl Into<String>) {
        self.step(key, StepStatus::Done, Some(detail.into()));
    }

    fn set_host(&self, host: &str) {
        let host = host.to_string();
        self.update(|h| h.host = Some(host));
    }

    fn finish_ok(&self) {
        self.update(|h| h.status = JobStatus::Done);
    }

    /// The failing step is Failed with the message; every step not yet Done is Skipped.
    fn finish_failed(&self, key: BootstrapStepKey, msg: String) {
        self.update(|h| {
            for s in &mut h.steps {
                if s.key == key {
                    s.status = StepStatus::Failed;
                    s.detail = Some(msg.clone());
                } else if s.status != StepStatus::Done {
                    s.status = StepStatus::Skipped;
                }
            }
            h.status = JobStatus::Failed;
            h.error = Some(msg);
        });
    }
}

async fn run_host(p: &Progress, address: &str, input: &SshBootstrapInput, ctx: &BootstrapCtx) {
    match run_steps(p, address, input, ctx).await {
        Ok(()) => p.finish_ok(),
        Err((key, msg)) => p.finish_failed(key, msg),
    }
}

// ---------------------------------------------------------------------------------------------
// The six steps
// ---------------------------------------------------------------------------------------------

async fn run_steps(
    p: &Progress,
    address: &str,
    input: &SshBootstrapInput,
    ctx: &BootstrapCtx,
) -> Result<(), StepErr> {
    use BootstrapStepKey as K;

    // A single-address job may name the host up front; show it before Connect even starts.
    let chosen_name = input
        .host_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(h) = &chosen_name {
        p.set_host(h);
    }
    // Nothing is worth uploading if the agent has no URL to dial back.
    let origin = ctx.server_origin.trim().trim_end_matches('/').to_string();
    if origin.is_empty() {
        return Err((K::Configure, "this server's origin is not configured — set [deploy].server_origin to the URL agents should dial, then retry".into()));
    }

    // 1. Connect
    p.running(K::Connect);
    let (host_part, port) = split_address(address, input.port);
    let ssh = Ssh::connect(&host_part, port, input.username.trim(), &input.auth)
        .await
        .map_err(|e| (K::Connect, e))?;
    p.done(
        K::Connect,
        format!(
            "{}@{host_part}:{port} · host key {}",
            input.username.trim(),
            ssh.fingerprint.as_deref().unwrap_or("unknown")
        ),
    );

    let result = run_after_connect(p, &ssh, chosen_name, &origin, input, ctx).await;
    ssh.close().await;
    result
}

async fn run_after_connect(
    p: &Progress,
    ssh: &Ssh,
    chosen_name: Option<String>,
    origin: &str,
    input: &SshBootstrapInput,
    ctx: &BootstrapCtx,
) -> Result<(), StepErr> {
    use BootstrapStepKey as K;

    // 2. Detect
    p.running(K::Detect);
    let out = ssh
        .exec(
            &format!("sh -c {}", sh_quote(DETECT_SCRIPT)),
            None,
            COMMAND_TIMEOUT,
        )
        .await
        .map_err(|e| (K::Detect, e))?;
    let detected = parse_detect(&out.stdout);
    if detected.arch.is_empty() || detected.hostname.is_empty() {
        return Err((
            K::Detect,
            format!("could not read the host's facts: {}", out.tail()),
        ));
    }
    let ssh_password = match &input.auth {
        SshAuth::Password { password } => Some(password.as_str()),
        _ => None,
    };
    let (privilege, sudo_stdin) = detected
        .privilege(ssh_password)
        .map_err(|e| (K::Detect, e))?;
    let host = chosen_name.unwrap_or_else(|| detected.hostname.clone());
    p.set_host(&host);
    let mut detail = detected.summary();
    if !detected.docker {
        detail.push_str(
            " · warning: docker is not installed — the agent will report host metrics only",
        );
    }
    p.done(K::Detect, detail);

    // 3. Push
    p.running(K::Push);
    let arch = normalize_arch(&detected.arch);
    let Some(local) = ctx.api.agent_binary_path(&arch) else {
        return Err((K::Push, format!("no agent binary for {arch} on this server — build photon-agent for {arch} and place it under the agent binaries directory")));
    };
    let bytes = tokio::fs::read(&local)
        .await
        .map_err(|e| (K::Push, format!("reading {}: {e}", local.display())))?;
    let digest = sha256_hex(&bytes);
    let tmp = format!(
        "/tmp/photon-agent.{}",
        new_id("bs").trim_start_matches("bs_")
    );
    let size = bytes.len();
    let started = Instant::now();
    let out = ssh
        .exec(
            &format!(
                "sh -c {}",
                sh_quote(&format!("umask 077 && cat > {}", sh_quote(&tmp)))
            ),
            Some(bytes),
            PUSH_TIMEOUT,
        )
        .await
        .map_err(|e| (K::Push, e))?;
    if !out.ok() {
        return Err((K::Push, format!("writing {tmp}: {}", out.tail())));
    }
    let out = ssh
        .exec(
            &format!(
                "sh -c {}",
                sh_quote(&format!(
                    "chmod 755 {t} && (sha256sum {t} 2>/dev/null || shasum -a 256 {t})",
                    t = sh_quote(&tmp)
                ))
            ),
            None,
            COMMAND_TIMEOUT,
        )
        .await
        .map_err(|e| (K::Push, e))?;
    let remote_digest = out
        .stdout
        .split_whitespace()
        .find(|w| w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_string);
    match remote_digest {
        Some(d) if d == digest => {}
        Some(d) => {
            let _ = ssh
                .exec(&format!("rm -f {}", sh_quote(&tmp)), None, COMMAND_TIMEOUT)
                .await;
            return Err((
                K::Push,
                format!(
                    "upload corrupted: sha256 {} on the host, {} here",
                    &d[..12],
                    &digest[..12]
                ),
            ));
        }
        None => {
            let _ = ssh
                .exec(&format!("rm -f {}", sh_quote(&tmp)), None, COMMAND_TIMEOUT)
                .await;
            return Err((
                K::Push,
                format!(
                    "could not verify the upload (no sha256sum on the host?): {}",
                    out.tail()
                ),
            ));
        }
    }
    p.done(
        K::Push,
        format!(
            "{} for {arch} in {:.1}s · sha256 {}",
            human_bytes(size),
            started.elapsed().as_secs_f64(),
            &digest[..12]
        ),
    );

    // 4. Configure
    p.running(K::Configure);
    let (token, how) = enrol_or_rotate(ctx, &host, input)
        .await
        .map_err(|e| (K::Configure, e))?;
    let args = install_args(
        origin,
        &token,
        &host,
        &input.capabilities,
        input.poll_secs.unwrap_or(DEFAULT_POLL_SECS),
    );
    let cmd = format!(
        "{privilege}{} {}",
        sh_quote(&tmp),
        args.iter()
            .map(|a| sh_quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let install = ssh.exec(&cmd, sudo_stdin, INSTALL_TIMEOUT).await;
    // The temp file goes whatever happened; the install copied it to /usr/local/bin.
    let _ = ssh
        .exec(&format!("rm -f {}", sh_quote(&tmp)), None, COMMAND_TIMEOUT)
        .await;
    let install = install.map_err(|e| (K::Configure, redact(&e, &token)))?;
    if !install.ok() {
        return Err((
            K::Configure,
            format!(
                "photon-agent install failed: {}",
                redact(&install.tail(), &token)
            ),
        ));
    }
    let last = install
        .stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    p.done(K::Configure, format!("{how} · {}", redact(last, &token)));

    // 5. Start
    p.running(K::Start);
    if !detected.systemd || !detected.systemctl {
        p.done(
            K::Start,
            "no systemd; started nothing — see the unit printed by install",
        );
    } else {
        let started = Instant::now();
        loop {
            let out = ssh
                .exec("systemctl is-active photon-agent", None, COMMAND_TIMEOUT)
                .await
                .map_err(|e| (K::Start, e))?;
            let state = out.stdout.trim().to_string();
            if state == "active" {
                p.done(
                    K::Start,
                    format!(
                        "photon-agent.service active after {:.0}s",
                        started.elapsed().as_secs_f64()
                    ),
                );
                break;
            }
            if started.elapsed() >= START_TIMEOUT {
                let status = ssh
                    .exec(
                        "systemctl status --no-pager -l photon-agent 2>&1 | tail -n 8",
                        None,
                        COMMAND_TIMEOUT,
                    )
                    .await
                    .map(|o| o.stdout)
                    .unwrap_or_default();
                return Err((
                    K::Start,
                    format!(
                        "photon-agent.service is {} after {}s: {}",
                        if state.is_empty() {
                            "not running"
                        } else {
                            state.as_str()
                        },
                        START_TIMEOUT.as_secs(),
                        redact(status.trim(), &token)
                    ),
                ));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    // 6. Sync
    p.running(K::Sync);
    let started = Instant::now();
    loop {
        if let Ok(Some(rec)) = ctx.api.store.get_host(&host).await {
            if rec.last_seen_ms.is_some() {
                p.done(
                    K::Sync,
                    format!("first sync after {:.0}s", started.elapsed().as_secs_f64()),
                );
                return Ok(());
            }
        }
        if started.elapsed() >= SYNC_TIMEOUT {
            return Err((
                K::Sync,
                format!("agent installed but never synced (check the host can reach {origin})"),
            ));
        }
        tokio::time::sleep(SYNC_POLL).await;
    }
}

/// Mint the token the install will carry: a fresh enrolment, or — when the host is already
/// enrolled (a re-bootstrap after a failed run, or a reinstall) — a rotation of its token that
/// also refreshes what the operator asked for this time. Returns `(token, what happened)`.
///
/// Goes through the store rather than `DeployApi::enroll` because that takes a `CurrentUser`,
/// which lives in photon-api's private `auth` module; the record written here has exactly the
/// shape `enroll` writes, and the audit rows carry `ACTOR` with the admin role.
async fn enrol_or_rotate(
    ctx: &BootstrapCtx,
    host: &str,
    input: &SshBootstrapInput,
) -> Result<(String, &'static str), String> {
    if !valid_host_name(host) {
        return Err(format!("host name {host:?} must be 1–253 characters of letters, digits, '-', '.' or '_' — give one explicitly"));
    }
    let site = input
        .site
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let token = mint_token();
    let existing = ctx
        .api
        .store
        .get_host(host)
        .await
        .map_err(|e| format!("reading {host}: {e}"))?;
    let (rec, action, detail, how) = match existing {
        None => {
            let rec = HostRecord {
                id: new_id("mh"),
                host: host.to_string(),
                site,
                token_hash: sha256_hex(token.as_bytes()),
                requested_capabilities: normalize_caps(&input.capabilities),
                reported_capabilities: vec![],
                agent_version: None,
                upgrade_to: None,
                poll_secs: input
                    .poll_secs
                    .unwrap_or(DEFAULT_POLL_SECS)
                    .max(MIN_POLL_SECS),
                last_seen_ms: None,
                enrolled_ms: now_ms(),
                docker: None,
                host_id: None,
                arch: None,
                os: None,
                labels: input.labels.clone(),
                desired_generation: 0,
            };
            let caps = rec
                .requested_capabilities
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            (
                rec,
                "agent.enroll",
                format!("over SSH; capabilities: {caps}"),
                "enrolled",
            )
        }
        Some(mut rec) => {
            rec.token_hash = sha256_hex(token.as_bytes());
            rec.requested_capabilities = normalize_caps(&input.capabilities);
            if let Some(poll) = input.poll_secs {
                rec.poll_secs = poll.max(MIN_POLL_SECS);
            }
            if site.is_some() {
                rec.site = site;
            }
            rec.labels
                .extend(input.labels.iter().map(|(k, v)| (k.clone(), v.clone())));
            (rec, "agent.rotate_token", "re-bootstrapped over SSH; the previous token stops working at the agent's next sync".to_string(), "already enrolled — token rotated")
        }
    };
    ctx.api
        .store
        .put_host(&rec)
        .await
        .map_err(|e| format!("saving {host}: {e}"))?;
    ctx.api
        .audit_system(
            ACTOR,
            Role::Admin,
            action,
            AuditTarget {
                host: Some(host.to_string()),
                ..Default::default()
            },
            AuditResultKind::Ok,
            Some(detail),
        )
        .await;
    Ok((token, how))
}

/// The same rule as enrolment's (`agents::valid_host_name`, which is private).
fn valid_host_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
}

// ---------------------------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------------------------

/// The argument list for `photon-agent install`: the server to dial, the token, the host name,
/// one `--docker-*` flag per requested capability and the poll interval. Same normalisation as
/// enrolment — asking for any Docker capability implies `read`.
pub fn install_args(
    server_origin: &str,
    token: &str,
    host_name: &str,
    capabilities: &[AgentCapability],
    poll_secs: u32,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "install".into(),
        "--server".into(),
        server_origin.trim_end_matches('/').to_string(),
        "--token".into(),
        token.to_string(),
        "--host-name".into(),
        host_name.to_string(),
    ];
    for cap in normalize_caps(capabilities) {
        args.push(
            match cap {
                AgentCapability::Read => "--docker-read",
                AgentCapability::Lifecycle => "--docker-lifecycle",
                AgentCapability::Apply => "--docker-apply",
                AgentCapability::Exec => "--docker-exec",
            }
            .into(),
        );
    }
    args.push("--poll".into());
    args.push(format!(
        "{}s",
        poll_secs.max(photon_api::deploy::agents::MIN_POLL_SECS)
    ));
    args
}

/// `agents::normalize_caps` is private to photon-api; the rule is one line.
fn normalize_caps(caps: &[AgentCapability]) -> Vec<AgentCapability> {
    let mut v = caps.to_vec();
    if !v.is_empty() && !v.contains(&AgentCapability::Read) {
        v.push(AgentCapability::Read);
    }
    v.sort();
    v.dedup();
    v
}

/// What Detect learned about the host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detected {
    /// `uname -m`: `x86_64`, `aarch64`, `armv7l`…
    pub arch: String,
    /// `uname -s`: `Linux`.
    pub os: String,
    /// `PRETTY_NAME` from `/etc/os-release`, when there is one.
    pub os_name: Option<String>,
    pub hostname: String,
    pub uid: Option<u32>,
    pub docker: bool,
    pub systemctl: bool,
    /// `/run/systemd/system` exists — systemd is PID 1, not merely installed.
    pub systemd: bool,
    pub sudo: bool,
    /// Root, or `sudo -n true` succeeded.
    pub sudo_nopass: bool,
}

impl Detected {
    pub fn is_root(&self) -> bool {
        self.uid == Some(0)
    }

    /// The command prefix that gets root plus what to feed it on stdin, or why there is none.
    /// Root needs nothing; passwordless sudo gets `sudo -n`; a user whose sudo wants a password
    /// gets `sudo -S` fed the SSH password (the common shape of a customer's ops account —
    /// refusing it would make the whole SSH path useless there). The password is read from
    /// stdin once and never appears on a command line.
    pub fn privilege(&self, password: Option<&str>) -> Result<(String, Option<Vec<u8>>), String> {
        if self.is_root() {
            Ok((String::new(), None))
        } else if self.sudo_nopass {
            Ok(("sudo -n ".into(), None))
        } else if self.sudo {
            match password {
                Some(pw) => Ok((
                    "sudo -S -p '' ".into(),
                    Some(format!("{pw}\n").into_bytes()),
                )),
                None => Err(format!("needs root or passwordless sudo: uid {} on the host can run sudo only with a password, and key auth gives none to offer — use password auth or grant NOPASSWD", self.uid.map(|u| u.to_string()).unwrap_or_else(|| "?".into()))),
            }
        } else {
            Err(format!(
                "needs root or passwordless sudo: uid {} on the host and no sudo installed",
                self.uid
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| "?".into())
            ))
        }
    }

    /// One line for the step detail: `x86_64 · Ubuntu 22.04.4 LTS · web-1 · root`.
    pub fn summary(&self) -> String {
        let os = self.os_name.clone().unwrap_or_else(|| self.os.clone());
        let who = if self.is_root() {
            "root".to_string()
        } else {
            format!(
                "uid {} via sudo",
                self.uid
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| "?".into())
            )
        };
        let mut s = format!("{} · {os} · {} · {who}", self.arch, self.hostname);
        if !self.systemd {
            s.push_str(" · no systemd");
        }
        s
    }
}

/// Parse `DETECT_SCRIPT`'s `key=value` lines. Unknown keys are ignored, missing ones default.
pub fn parse_detect(output: &str) -> Detected {
    let mut d = Detected::default();
    let yes = |v: &str| v.trim() == "yes";
    for line in output.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "arch" => d.arch = v.to_string(),
            "os" => d.os = v.to_string(),
            "hostname" => d.hostname = v.to_string(),
            "uid" => d.uid = v.parse().ok(),
            "pretty" => d.os_name = Some(v.to_string()).filter(|s| !s.is_empty()),
            "docker" => d.docker = yes(v),
            "systemctl" => d.systemctl = yes(v),
            "systemd" => d.systemd = yes(v),
            "sudo" => d.sudo = yes(v),
            "sudo_nopass" => d.sudo_nopass = yes(v),
            _ => {}
        }
    }
    d
}

/// `host`, `host:port`, `[v6]`, `[v6]:port` → `(host, port)`; a bare IPv6 literal keeps the
/// default port (its colons are the address).
pub fn split_address(address: &str, default_port: u16) -> (String, u16) {
    let a = address.trim();
    if let Some(rest) = a.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            let port = tail
                .strip_prefix(':')
                .and_then(|p| p.parse().ok())
                .unwrap_or(default_port);
            return (host.to_string(), port);
        }
    }
    if a.matches(':').count() == 1 {
        if let Some((host, port)) = a.split_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                return (host.to_string(), port);
            }
        }
    }
    (a.to_string(), default_port)
}

/// POSIX single-quote quoting: safe for any byte sequence, including the empty string.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Remove the agent token from anything shown to the operator or written to a log.
pub fn redact(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_string()
    } else {
        text.replace(token, "ak_[redacted]")
    }
}

fn human_bytes(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

// ---------------------------------------------------------------------------------------------
// The SSH client
// ---------------------------------------------------------------------------------------------

/// Accepts every host key and remembers its fingerprint. Trust-on-first-use from the control
/// plane: the operator just typed the address and the credentials, and the fingerprint lands
/// in the Connect step so it can be compared afterwards.
struct AcceptAnyHostKey {
    fingerprint: Arc<Mutex<Option<String>>>,
}

impl Handler for AcceptAnyHostKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fp = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        *self.fingerprint.lock().unwrap_or_else(|e| e.into_inner()) = Some(fp);
        Ok(true)
    }
}

/// The output of one remote command.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<u32>,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.exit == Some(0)
    }

    /// The last few lines of whatever the command said, for an error message.
    pub fn tail(&self) -> String {
        let src = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let lines: Vec<&str> = src.lines().filter(|l| !l.trim().is_empty()).collect();
        let n = lines.len();
        let text = lines[n.saturating_sub(4)..].join(" / ");
        match self.exit {
            Some(0) | None if text.is_empty() => "(no output)".into(),
            Some(code) if text.is_empty() => format!("exit {code}"),
            Some(code) if code != 0 => format!("exit {code}: {text}"),
            _ => text,
        }
    }
}

/// One authenticated session.
struct Ssh {
    handle: Handle<AcceptAnyHostKey>,
    fingerprint: Option<String>,
}

impl Ssh {
    async fn connect(host: &str, port: u16, username: &str, auth: &SshAuth) -> Result<Ssh, String> {
        let fingerprint = Arc::new(Mutex::new(None));
        let handler = AcceptAnyHostKey {
            fingerprint: fingerprint.clone(),
        };
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(15 * 60)),
            keepalive_interval: Some(Duration::from_secs(15)),
            ..Default::default()
        });
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let mut handle = client::connect(config, (host, port), handler)
                .await
                .map_err(|e| format!("{host}:{port}: {}", ssh_err(&e)))?;
            authenticate(&mut handle, username, auth).await?;
            Ok::<_, String>(handle)
        })
        .await;
        let handle = match connected {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(format!(
                    "{host}:{port}: no SSH session after {}s",
                    CONNECT_TIMEOUT.as_secs()
                ))
            }
        };
        let fp = fingerprint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Ok(Ssh {
            handle,
            fingerprint: fp,
        })
    }

    /// Run `command` (the remote user's shell parses it), optionally feeding `stdin`, and
    /// collect stdout / stderr / exit status. `stdin` is streamed under the channel window, so a
    /// multi-megabyte binary is fine.
    async fn exec(
        &self,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<Output, String> {
        let run = async {
            let mut ch = self
                .handle
                .channel_open_session()
                .await
                .map_err(|e| format!("opening a channel: {}", ssh_err(&e)))?;
            ch.exec(true, command)
                .await
                .map_err(|e| format!("exec: {}", ssh_err(&e)))?;
            if let Some(bytes) = stdin {
                ch.data(&bytes[..])
                    .await
                    .map_err(|e| format!("writing stdin: {}", ssh_err(&e)))?;
            }
            ch.eof()
                .await
                .map_err(|e| format!("closing stdin: {}", ssh_err(&e)))?;
            let mut out = Output::default();
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            while let Some(msg) = ch.wait().await {
                match msg {
                    ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
                    ChannelMsg::ExitStatus { exit_status } => out.exit = Some(exit_status),
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            out.stdout = String::from_utf8_lossy(&stdout).into_owned();
            out.stderr = String::from_utf8_lossy(&stderr).into_owned();
            Ok::<_, String>(out)
        };
        match tokio::time::timeout(timeout, run).await {
            Ok(r) => r,
            Err(_) => Err(format!(
                "remote command did not finish within {}s",
                timeout.as_secs()
            )),
        }
    }

    async fn close(&self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }
}

/// Password (with a keyboard-interactive fallback, for an sshd that only allows the latter) or
/// private key + optional passphrase.
async fn authenticate(
    handle: &mut Handle<AcceptAnyHostKey>,
    username: &str,
    auth: &SshAuth,
) -> Result<(), String> {
    match auth {
        SshAuth::Password { password } => {
            let res = handle
                .authenticate_password(username, password.clone())
                .await
                .map_err(|e| format!("password authentication: {}", ssh_err(&e)))?;
            if res.success() {
                return Ok(());
            }
            let allows_kbd = match &res {
                russh::client::AuthResult::Failure {
                    remaining_methods, ..
                } => remaining_methods
                    .iter()
                    .any(|m| matches!(m, russh::MethodKind::KeyboardInteractive)),
                _ => false,
            };
            if allows_kbd && keyboard_interactive(handle, username, password).await? {
                return Ok(());
            }
            Err(format!(
                "authentication as {username} failed{}",
                remaining(&res)
            ))
        }
        SshAuth::Key {
            private_key,
            passphrase,
        } => {
            let key = decode_secret_key(
                private_key,
                passphrase.as_deref().filter(|p| !p.is_empty()),
            )
            .map_err(|e| {
                format!("private key: {e} (OpenSSH, PKCS#8, PEM and PuTTY formats are accepted)")
            })?;
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| ssh_err(&e))?
                .flatten();
            let res = handle
                .authenticate_publickey(username, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(|e| format!("public-key authentication: {}", ssh_err(&e)))?;
            if res.success() {
                Ok(())
            } else {
                Err(format!(
                    "the key was not accepted for {username}{}",
                    remaining(&res)
                ))
            }
        }
    }
}

/// Answer every keyboard-interactive prompt with the password; `Ok(false)` when the server
/// still says no.
async fn keyboard_interactive(
    handle: &mut Handle<AcceptAnyHostKey>,
    username: &str,
    password: &str,
) -> Result<bool, String> {
    let mut res = handle
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(|e| format!("keyboard-interactive authentication: {}", ssh_err(&e)))?;
    for _ in 0..4 {
        match res {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                res = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|e| format!("keyboard-interactive authentication: {}", ssh_err(&e)))?;
            }
        }
    }
    Ok(false)
}

fn remaining(res: &russh::client::AuthResult) -> String {
    match res {
        russh::client::AuthResult::Failure {
            remaining_methods, ..
        } if !remaining_methods.is_empty() => {
            format!(
                " (the server accepts: {})",
                remaining_methods
                    .iter()
                    .map(|m| format!("{m:?}").to_lowercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        _ => String::new(),
    }
}

fn ssh_err(e: &russh::Error) -> String {
    match e {
        russh::Error::IO(io) => io.to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_args_carry_the_flags_per_capability() {
        let args = install_args(
            "https://photon.example/",
            "ak_abc",
            "web-1",
            &[AgentCapability::Lifecycle],
            30,
        );
        assert_eq!(
            args,
            vec![
                "install",
                "--server",
                "https://photon.example",
                "--token",
                "ak_abc",
                "--host-name",
                "web-1",
                "--docker-read",
                "--docker-lifecycle",
                "--poll",
                "30s"
            ]
        );
        // No capabilities → a metrics-only agent, no docker flags at all.
        let args = install_args("http://10.0.0.1:8080", "ak_x", "h", &[], 15);
        assert!(!args.iter().any(|a| a.starts_with("--docker")));
        assert_eq!(&args[args.len() - 2..], ["--poll", "15s"]);
        // The floor on the poll interval is the same as enrolment's.
        assert_eq!(install_args("o", "t", "h", &[], 1).last().unwrap(), "5s");
        // Every capability, deduplicated and ordered.
        let args = install_args(
            "o",
            "t",
            "h",
            &[
                AgentCapability::Exec,
                AgentCapability::Apply,
                AgentCapability::Exec,
                AgentCapability::Read,
            ],
            15,
        );
        let flags: Vec<&str> = args
            .iter()
            .filter(|a| a.starts_with("--docker"))
            .map(String::as_str)
            .collect();
        assert_eq!(flags, ["--docker-read", "--docker-apply", "--docker-exec"]);
    }

    #[test]
    fn parse_detect_reads_every_fact() {
        let out = "arch=x86_64\nos=Linux\nhostname=web-1\nuid=1000\npretty=Ubuntu 22.04.4 LTS\ndocker=yes\nsystemctl=yes\nsudo=yes\nsystemd=yes\nsudo_nopass=yes\n";
        let d = parse_detect(out);
        assert_eq!(
            d,
            Detected {
                arch: "x86_64".into(),
                os: "Linux".into(),
                os_name: Some("Ubuntu 22.04.4 LTS".into()),
                hostname: "web-1".into(),
                uid: Some(1000),
                docker: true,
                systemctl: true,
                systemd: true,
                sudo: true,
                sudo_nopass: true,
            }
        );
        assert!(!d.is_root());
        assert_eq!(d.privilege(None).unwrap().0, "sudo -n ");
        assert_eq!(
            d.summary(),
            "x86_64 · Ubuntu 22.04.4 LTS · web-1 · uid 1000 via sudo"
        );

        // Root on a box without os-release, no docker, no systemd: falls back to `uname -s`.
        let d = parse_detect("arch=aarch64\nos=Linux\nhostname=pi\nuid=0\npretty=\ndocker=no\nsystemctl=no\nsudo=no\nsystemd=no\nsudo_nopass=yes\nunknown=ignored\n");
        assert!(d.is_root());
        assert_eq!(d.os_name, None);
        assert_eq!(d.privilege(None).unwrap().0, "");
        assert_eq!(d.summary(), "aarch64 · Linux · pi · root · no systemd");

        // Unprivileged without passwordless sudo is refused with a clear reason.
        let d = parse_detect("arch=x86_64\nhostname=h\nuid=1000\nsudo=yes\nsudo_nopass=no\n");
        let e = d.privilege(None).unwrap_err();
        let (prefix, stdin) = d.privilege(Some("s3cret")).unwrap();
        assert_eq!(prefix, "sudo -S -p '' ");
        assert_eq!(stdin.as_deref(), Some(&b"s3cret\n"[..]));
        assert!(e.starts_with("needs root or passwordless sudo"), "{e}");
        let d = parse_detect("arch=x86_64\nhostname=h\nuid=1000\nsudo=no\nsudo_nopass=no\n");
        assert!(d.privilege(None).unwrap_err().contains("no sudo installed"));

        // Garbage in → empty facts, which the Detect step turns into a failure.
        let d = parse_detect("bash: uname: command not found");
        assert!(d.arch.is_empty() && d.hostname.is_empty());
    }

    #[test]
    fn addresses_split_on_an_explicit_port() {
        assert_eq!(split_address("10.0.0.5", 22), ("10.0.0.5".into(), 22));
        assert_eq!(
            split_address(" 10.0.0.5:2222 ", 22),
            ("10.0.0.5".into(), 2222)
        );
        assert_eq!(split_address("web-1.lan:x", 22), ("web-1.lan:x".into(), 22));
        assert_eq!(split_address("fe80::1", 22), ("fe80::1".into(), 22));
        assert_eq!(split_address("[fe80::1]", 22), ("fe80::1".into(), 22));
        assert_eq!(
            split_address("[fe80::1]:2200", 22),
            ("fe80::1".into(), 2200)
        );
    }

    #[test]
    fn quoting_and_redaction() {
        assert_eq!(sh_quote("plain"), "'plain'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(
            redact("--token ak_abc123 --poll", "ak_abc123"),
            "--token ak_[redacted] --poll"
        );
        assert_eq!(redact("nothing", ""), "nothing");
        assert_eq!(human_bytes(15 * 1024 * 1024 + 300 * 1024), "15.3 MiB");
        assert_eq!(human_bytes(2048), "2 KiB");
        assert_eq!(human_bytes(12), "12 B");
    }

    #[test]
    fn output_tail_summarizes_failures() {
        let o = Output {
            stdout: "a\nb\nc\nd\ne\n".into(),
            stderr: String::new(),
            exit: Some(1),
        };
        assert_eq!(o.tail(), "exit 1: b / c / d / e");
        let o = Output {
            stdout: "ignored".into(),
            stderr: "boom".into(),
            exit: Some(2),
        };
        assert_eq!(o.tail(), "exit 2: boom");
        let o = Output {
            stdout: String::new(),
            stderr: String::new(),
            exit: None,
        };
        assert_eq!(o.tail(), "(no output)");
        let o = Output {
            stdout: "fine".into(),
            stderr: String::new(),
            exit: Some(0),
        };
        assert_eq!(o.tail(), "fine");
        assert!(o.ok());
    }

    #[tokio::test]
    async fn progress_marks_failed_step_and_skips_the_rest() {
        let hub = Arc::new(DeployHub::new());
        hub.put_job(BootstrapJob {
            id: "bj_1".into(),
            status: JobStatus::Running,
            started_ms: 0,
            credentials_kept: false,
            hosts: vec![
                BootstrapHostProgress::new("10.0.0.1"),
                BootstrapHostProgress::new("10.0.0.2"),
            ],
        });
        let p = Progress {
            hub: hub.clone(),
            job_id: "bj_1".into(),
            idx: 1,
        };
        p.running(BootstrapStepKey::Connect);
        p.done(BootstrapStepKey::Connect, "ok");
        p.set_host("web-2");
        p.running(BootstrapStepKey::Detect);
        p.finish_failed(
            BootstrapStepKey::Detect,
            "needs root or passwordless sudo".into(),
        );
        let job = hub.job("bj_1").unwrap();
        // The other host is untouched.
        assert_eq!(job.hosts[0], BootstrapHostProgress::new("10.0.0.1"));
        let h = &job.hosts[1];
        assert_eq!(h.host.as_deref(), Some("web-2"));
        assert_eq!(h.status, JobStatus::Failed);
        assert_eq!(h.error.as_deref(), Some("needs root or passwordless sudo"));
        let statuses: Vec<StepStatus> = h.steps.iter().map(|s| s.status).collect();
        assert_eq!(
            statuses,
            [
                StepStatus::Done,
                StepStatus::Failed,
                StepStatus::Skipped,
                StepStatus::Skipped,
                StepStatus::Skipped,
                StepStatus::Skipped
            ]
        );
        assert_eq!(h.steps[0].detail.as_deref(), Some("ok"));
        assert_eq!(
            h.steps[1].detail.as_deref(),
            Some("needs root or passwordless sudo")
        );
    }

    /// Connect + Detect against this machine's own sshd with `~/.ssh/id_ed25519`. Never
    /// installs anything. `cargo test -p photon-server deploy::bootstrap -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn connect_and_detect_against_localhost() {
        let home = std::env::var("HOME").unwrap();
        let key =
            std::fs::read_to_string(format!("{home}/.ssh/id_ed25519")).expect("~/.ssh/id_ed25519");
        let user = std::env::var("USER").unwrap();
        let ssh = Ssh::connect(
            "127.0.0.1",
            22,
            &user,
            &SshAuth::Key {
                private_key: key,
                passphrase: None,
            },
        )
        .await
        .expect("connect");
        assert!(ssh
            .fingerprint
            .as_deref()
            .unwrap_or("")
            .starts_with("SHA256:"));
        let out = ssh
            .exec(
                &format!("sh -c {}", sh_quote(DETECT_SCRIPT)),
                None,
                COMMAND_TIMEOUT,
            )
            .await
            .expect("detect");
        assert!(out.ok(), "{}", out.tail());
        let d = parse_detect(&out.stdout);
        assert_eq!(d.arch, std::env::consts::ARCH);
        assert!(!d.hostname.is_empty());
        assert_eq!(d.os, "Linux");
        // stdin streaming, the way Push uses it.
        let payload = vec![7u8; 3 * 1024 * 1024];
        let out = ssh
            .exec("sha256sum", Some(payload.clone()), PUSH_TIMEOUT)
            .await
            .expect("sha256sum");
        assert!(
            out.stdout.starts_with(&sha256_hex(&payload)),
            "{}",
            out.stdout
        );
        ssh.close().await;
    }
}
