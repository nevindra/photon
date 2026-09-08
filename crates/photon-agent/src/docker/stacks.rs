//! The `apply` capability (R4.1): a desired stack becomes a folder under
//! `{data_dir}/stacks/{name}/` — the rendered files exactly as the server sent them, plus the
//! `io.photon.stack` label injected into every service and the secret values appended to
//! `.env` (mode 0600, fetched at apply time by the caller, never persisted anywhere else) —
//! and then `docker compose -p NAME --project-directory DIR -f FILE <op>` runs against it.
//! Compose is driven through its CLI on purpose: it is the one implementation of compose
//! semantics that matches what an operator gets by hand, and `docker compose` v2 ships with
//! every supported Docker.
//!
//! The pure decisions (which file, which args, path safety, label injection) live in
//! `compose.rs`; this file is the part that touches the disk and the daemon.
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bollard::query_parameters::ListContainersOptionsBuilder;
use bollard::Docker;
use photon_deploy::model::StackOp;
use photon_deploy::proto::{DesiredStack, RegistryAuth};

use super::compose::{
    compose_commands, compose_file, docker_config_json, inject_stack_label, safe_relative,
    truncate_tail, valid_project_name,
};
use super::inventory::{LABEL_CONFIG_HASH, LABEL_PROJECT, LABEL_SERVICE};

/// A single compose command may legitimately pull gigabytes over a slow customer link.
pub const COMPOSE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Transcript kept for the result detail — the tail, where compose puts its errors.
pub const DETAIL_MAX: usize = 4096;

pub fn stack_dir(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join("stacks").join(name)
}

/// `.env` with the secret values merged in: an existing `NAME=` line for a secret is replaced
/// (a name-only placeholder is what the server ships), everything else is kept verbatim.
pub fn merge_env(existing: &str, secrets: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for line in existing.lines() {
        let key = line.trim_start().split(['=', ' ']).next().unwrap_or("");
        if !key.is_empty() && !line.trim_start().starts_with('#') && secrets.contains_key(key) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    for (k, v) in secrets {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

/// Write the stack folder. Returns the compose file path inside it.
pub fn write_files(
    dir: &Path,
    stack: &DesiredStack,
    secrets: &BTreeMap<String, String>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    set_mode(dir, 0o700);
    let compose = compose_file(&stack.files).ok_or("desired stack has no compose file")?;
    let mut wrote_env = false;
    for f in &stack.files {
        let rel = safe_relative(&f.path)?;
        let path = dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        let is_env = rel == Path::new(".env");
        let content = if f.path == compose.path {
            inject_stack_label(&f.content, &stack.id)?
        } else if is_env {
            wrote_env = true;
            merge_env(&f.content, secrets)
        } else {
            f.content.clone()
        };
        std::fs::write(&path, content).map_err(|e| format!("writing {}: {e}", path.display()))?;
        set_mode(&path, if is_env { 0o600 } else { 0o644 });
    }
    if !wrote_env && !secrets.is_empty() {
        let path = dir.join(".env");
        std::fs::write(&path, merge_env("", secrets))
            .map_err(|e| format!("writing {}: {e}", path.display()))?;
        set_mode(&path, 0o600);
    }
    Ok(dir.join(safe_relative(&compose.path)?))
}

/// Where the Docker CLI would read its config from for this process: `$DOCKER_CONFIG`, else
/// `$HOME/.docker`, else root's (a systemd service without `User=` has no `$HOME`, and the
/// CLI falls back to the passwd entry, which for uid 0 is `/root`).
fn host_docker_config_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("DOCKER_CONFIG") {
        if !d.is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return Some(Path::new(&h).join(".docker"));
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: getuid has no preconditions and cannot fail.
        if unsafe { libc::getuid() } == 0 {
            return Some(PathBuf::from("/root/.docker"));
        }
    }
    None
}

/// A throwaway `DOCKER_CONFIG` directory carrying the registry credentials for one compose
/// run, removed on drop. The credentials exist on disk only between the write and the end of
/// the apply, in a 0700 directory under the agent's own data dir.
pub struct TempDockerConfig {
    dir: PathBuf,
}

impl TempDockerConfig {
    pub fn write(data_dir: &Path, stack: &str, auths: &[RegistryAuth]) -> Result<Self, String> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = data_dir
            .join("registry-auth")
            .join(format!("{stack}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        set_mode(&dir, 0o700);
        let original = host_docker_config_dir();
        let existing = original
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("config.json")).ok());
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            docker_config_json(existing.as_deref(), original.as_deref(), auths),
        )
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
        set_mode(&path, 0o600);
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TempDockerConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// Run one `docker compose` invocation, returning the combined transcript (stdout then
/// stderr) or the same transcript as the error.
pub async fn run_compose(
    dir: &Path,
    name: &str,
    file: &Path,
    args: &[String],
    docker_config: Option<&Path>,
) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("docker");
    if let Some(cfg) = docker_config {
        cmd.env("DOCKER_CONFIG", cfg);
    }
    cmd.arg("compose")
        .arg("-p")
        .arg(name)
        .arg("--project-directory")
        .arg(dir)
        .arg("-f")
        .arg(file)
        .args(args)
        .current_dir(dir)
        .env("COMPOSE_PROGRESS", "plain")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    let line = format!("$ docker compose -p {name} {}\n", args.join(" "));
    let out = match tokio::time::timeout(COMPOSE_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("{line}could not run docker compose: {e}")),
        Err(_) => {
            return Err(format!(
                "{line}timed out after {}s",
                COMPOSE_TIMEOUT.as_secs()
            ))
        }
    };
    let mut transcript = line;
    transcript.push_str(&String::from_utf8_lossy(&out.stdout));
    transcript.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(transcript)
    } else {
        Err(format!(
            "{transcript}\nexit status: {}",
            out.status
                .code()
                .map(|c| c.to_string())
                .unwrap_or("signal".into())
        ))
    }
}

/// Apply one desired stack end to end. `Ok((detail, config_hashes))` or `Err(detail)`; the
/// detail is the (tail-truncated) compose transcript either way.
pub async fn apply(
    docker: Option<&Docker>,
    data_dir: &Path,
    stack: &DesiredStack,
    secrets: &BTreeMap<String, String>,
    auths: &[RegistryAuth],
) -> Result<(String, BTreeMap<String, String>), String> {
    if !valid_project_name(&stack.name) {
        return Err(format!(
            "'{}' is not a valid compose project name",
            stack.name
        ));
    }
    let dir = stack_dir(data_dir, &stack.name);
    let started = std::time::Instant::now();
    let file = if stack.op == StackOp::Destroy && !dir.exists() {
        // Nothing on disk to destroy: still tell compose to take the project down by name.
        write_files(&dir, stack, secrets)?
    } else {
        write_files(&dir, stack, secrets)?
    };
    let mut transcript = String::new();
    // Credentials only when there are any and something may get pulled — a destroy or a down
    // never pulls, and a host with no credentials keeps its own `docker login` state.
    let pulls = !matches!(stack.op, StackOp::Destroy | StackOp::Down);
    let cfg = if pulls && !auths.is_empty() {
        let c = TempDockerConfig::write(data_dir, &stack.name, auths)?;
        transcript.push_str(&format!(
            "(registry credentials for {} from Photon)\n",
            auths
                .iter()
                .map(|a| a.server.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        Some(c)
    } else {
        None
    };
    for args in compose_commands(
        stack.op,
        &stack.deploy_options,
        stack.remove_volumes,
        stack.prepull,
    ) {
        match run_compose(
            &dir,
            &stack.name,
            &file,
            &args,
            cfg.as_ref().map(|c| c.dir()),
        )
        .await
        {
            Ok(t) => transcript.push_str(&t),
            Err(t) => {
                transcript.push_str(&t);
                return Err(truncate_tail(&transcript, DETAIL_MAX));
            }
        }
    }
    let mut hashes = BTreeMap::new();
    match stack.op {
        StackOp::Destroy => {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                transcript.push_str(&format!("\n(folder {} not removed: {e})", dir.display()));
            }
        }
        StackOp::Down => {}
        _ => {
            if let Some(d) = docker {
                hashes = config_hashes(d, &stack.name).await.unwrap_or_default();
            }
        }
    }
    transcript.push_str(&format!(
        "\ndone in {:.1}s",
        started.elapsed().as_secs_f64()
    ));
    Ok((truncate_tail(&transcript, DETAIL_MAX), hashes))
}

/// `com.docker.compose.config-hash` per service for a project, read back from its containers
/// right after an apply — the drift baseline the server compares future inventories against.
pub async fn config_hashes(
    docker: &Docker,
    project: &str,
) -> Result<BTreeMap<String, String>, String> {
    let filter_value = format!("{LABEL_PROJECT}={project}");
    let filters: HashMap<&str, Vec<&str>> = HashMap::from([("label", vec![filter_value.as_str()])]);
    let list = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::new()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await
        .map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    for c in list {
        let labels = c.labels.unwrap_or_default();
        if let (Some(svc), Some(hash)) = (labels.get(LABEL_SERVICE), labels.get(LABEL_CONFIG_HASH))
        {
            out.insert(svc.clone(), hash.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use photon_deploy::model::{DeployOptions, StackFile};

    #[test]
    fn env_merge_replaces_placeholders_and_appends_secrets() {
        let secrets = BTreeMap::from([("DB_PASSWORD".to_string(), "s3cr3t".to_string())]);
        let out = merge_env("# comment\nPORT=8080\nDB_PASSWORD=\n", &secrets);
        assert_eq!(out, "# comment\nPORT=8080\nDB_PASSWORD=s3cr3t\n");
        assert_eq!(merge_env("", &secrets), "DB_PASSWORD=s3cr3t\n");
        assert_eq!(merge_env("A=1", &BTreeMap::new()), "A=1\n");
    }

    #[test]
    fn writes_the_folder_with_label_and_env_mode() {
        let base = std::env::temp_dir().join(format!("photon-agent-stack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let stack = DesiredStack {
            id: "stk_7".into(),
            name: "shop".into(),
            generation: 1,
            op: StackOp::Up,
            files: vec![
                StackFile {
                    path: "compose.yaml".into(),
                    content: "services:\n  web:\n    image: nginx\n".into(),
                },
                StackFile {
                    path: ".env".into(),
                    content: "PORT=80\nDB_PASSWORD=\n".into(),
                },
                StackFile {
                    path: "conf/nginx.conf".into(),
                    content: "x".into(),
                },
            ],
            secret_names: vec!["DB_PASSWORD".into()],
            deploy_options: DeployOptions::default(),
            remove_volumes: false,
            prepull: false,
        };
        let secrets = BTreeMap::from([("DB_PASSWORD".to_string(), "pw".to_string())]);
        let dir = stack_dir(&base, "shop");
        let file = write_files(&dir, &stack, &secrets).unwrap();
        assert_eq!(file, dir.join("compose.yaml"));
        let compose = std::fs::read_to_string(&file).unwrap();
        assert!(compose.contains("io.photon.stack: stk_7"));
        let env = std::fs::read_to_string(dir.join(".env")).unwrap();
        assert_eq!(env, "PORT=80\nDB_PASSWORD=pw\n");
        assert_eq!(
            std::fs::read_to_string(dir.join("conf/nginx.conf")).unwrap(),
            "x"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.join(".env"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        // An escaping path is refused before anything is written.
        let mut bad = stack.clone();
        bad.files.push(StackFile {
            path: "../evil".into(),
            content: "".into(),
        });
        assert!(write_files(&dir, &bad, &secrets)
            .unwrap_err()
            .contains("escapes"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
