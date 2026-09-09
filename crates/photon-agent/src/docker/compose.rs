//! The pure half of stack apply (R4.1): which file is the compose file, whether a stack path is
//! safe to write, how the `io.photon.stack` label gets into every service, and which arguments
//! each `StackOp` turns into. No process is spawned here — `stacks.rs` does that — so all of
//! it is table-testable.
//!
//! What this deliberately is NOT: a template engine. The files arrive rendered from the
//! server (R1.8); the one edit the agent makes is adding a label so every container Photon
//! creates can be traced back to its stack id from the inventory alone.
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use photon_deploy::model::{DeployOptions, PullPolicy, StackFile, StackOp};
use photon_deploy::proto::RegistryAuth;
use photon_deploy::registry::{config_key, normalize_server};

/// The Docker CLI `config.json` a compose run gets through `DOCKER_CONFIG` when the server
/// handed this host registry credentials: the host's own config (so its `credHelpers`,
/// proxies and CLI plugin settings survive) with our `auths` laid over it, plus the original
/// config dir's `cli-plugins` in `cliPluginsExtraDirs` — `docker compose` is itself a CLI
/// plugin, and a user-local install of it lives exactly there, which `DOCKER_CONFIG` would
/// otherwise hide. Written to a 0700 directory for the one compose run and removed after; the
/// host is never `docker login`-ed.
pub fn docker_config_json(
    existing: Option<&str>,
    original_dir: Option<&Path>,
    auths: &[RegistryAuth],
) -> String {
    use serde_json::{json, Map, Value};
    let mut root: Map<String, Value> = existing
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();
    let mut auth_map: Map<String, Value> = match root.remove("auths") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    for a in auths {
        let key = config_key(&normalize_server(&a.server));
        let token = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", a.username, a.secret));
        auth_map.insert(key, json!({ "auth": token }));
    }
    root.insert("auths".into(), Value::Object(auth_map));
    if let Some(dir) = original_dir {
        let plugins = dir.join("cli-plugins").to_string_lossy().to_string();
        let mut extra: Vec<Value> = match root.remove("cliPluginsExtraDirs") {
            Some(Value::Array(v)) => v,
            _ => Vec::new(),
        };
        if !extra.iter().any(|v| v.as_str() == Some(plugins.as_str())) {
            extra.push(Value::String(plugins));
        }
        root.insert("cliPluginsExtraDirs".into(), Value::Array(extra));
    }
    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_else(|_| "{}".into())
}

/// The label every container created by a Photon-managed stack carries.
pub const STACK_LABEL: &str = "io.photon.stack";

/// Docker's own list of recognised compose filenames, in its preference order.
pub const COMPOSE_NAMES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// Pick the compose file among a stack's files: Docker's names first, then any YAML at the
/// top level.
pub fn compose_file(files: &[StackFile]) -> Option<&StackFile> {
    for name in COMPOSE_NAMES {
        if let Some(f) = files.iter().find(|f| f.path == name) {
            return Some(f);
        }
    }
    files
        .iter()
        .find(|f| !f.path.contains('/') && (f.path.ends_with(".yaml") || f.path.ends_with(".yml")))
}

/// A stack file path must resolve INSIDE the stack folder (R4.2): relative, no `..`, no root,
/// no empty segments. Returns the normalised relative path.
pub fn safe_relative(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("empty path".into());
    }
    let p = Path::new(path);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(seg) => out.push(seg),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("path '{path}' escapes the stack folder ('..')"))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("path '{path}' is absolute"))
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(format!("path '{path}' names no file"));
    }
    Ok(out)
}

/// A compose project name doubles as a folder name under the data dir, so it gets the same
/// scrutiny: Docker's own rule (lowercase alphanumerics, `-`, `_`, must start alphanumeric).
pub fn valid_project_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Add `io.photon.stack: <id>` to every service's `labels`, whatever shape they are in
/// (mapping, `KEY=VALUE` list, or absent). YAML merge keys (`<<: *anchor`) are applied first so
/// the re-serialised document is self-contained.
pub fn inject_stack_label(yaml: &str, stack_id: &str) -> Result<String, String> {
    let mut doc: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| format!("compose file is not YAML: {e}"))?;
    doc.apply_merge()
        .map_err(|e| format!("compose file merge keys: {e}"))?;
    let Some(services) = doc.get_mut("services").and_then(|s| s.as_mapping_mut()) else {
        return Err("compose file has no `services` mapping".into());
    };
    let label_line = format!("{STACK_LABEL}={stack_id}");
    for (_, svc) in services.iter_mut() {
        let Some(svc) = svc.as_mapping_mut() else {
            continue;
        };
        let key = serde_yaml::Value::String("labels".into());
        match svc.get_mut(&key) {
            Some(serde_yaml::Value::Sequence(seq)) => {
                seq.retain(
                    |v| !matches!(v.as_str(), Some(s) if s.starts_with(&format!("{STACK_LABEL}="))),
                );
                seq.push(serde_yaml::Value::String(label_line.clone()));
            }
            Some(serde_yaml::Value::Mapping(map)) => {
                map.insert(
                    serde_yaml::Value::String(STACK_LABEL.into()),
                    serde_yaml::Value::String(stack_id.into()),
                );
            }
            _ => {
                let mut map = serde_yaml::Mapping::new();
                map.insert(
                    serde_yaml::Value::String(STACK_LABEL.into()),
                    serde_yaml::Value::String(stack_id.into()),
                );
                svc.insert(key, serde_yaml::Value::Mapping(map));
            }
        }
    }
    serde_yaml::to_string(&doc).map_err(|e| format!("re-serialising compose file: {e}"))
}

/// The `docker compose …` invocations for one op, AFTER the common
/// `-p NAME --project-directory DIR -f FILE` prefix. Several ops are two commands.
pub fn compose_commands(
    op: StackOp,
    opts: &DeployOptions,
    remove_volumes: bool,
    prepull: bool,
) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let pull_flag = match opts.pull {
        PullPolicy::Always => "always",
        PullPolicy::Missing => "missing",
        PullPolicy::Never => "never",
    };
    let mut cmds = Vec::new();
    match op {
        StackOp::Up => {
            if prepull && opts.pull != PullPolicy::Never {
                cmds.push(s(&["pull", "--ignore-buildable"]));
            }
            // "Recreate volumes" is the operator's data-loss choice, and it means NAMED
            // volumes: `up -V` only renews anonymous ones (a stack upgraded from postgres 16
            // to 17 kept its PG16 data under `-V` and crash-looped), so the project comes
            // down with `--volumes` first and then up fresh.
            if opts.recreate_volumes {
                cmds.push(s(&["down", "--remove-orphans", "--volumes"]));
            }
            let mut up = s(&["up", "-d", "--remove-orphans", "--pull", pull_flag]);
            if opts.force_recreate {
                up.push("--force-recreate".into());
            }
            cmds.push(up);
        }
        StackOp::Down => cmds.push(s(&["down"])),
        StackOp::Restart => cmds.push(s(&["restart"])),
        StackOp::Redeploy => {
            cmds.push(s(&["pull", "--ignore-buildable"]));
            cmds.push(s(&["up", "-d", "--force-recreate", "--remove-orphans"]));
        }
        StackOp::Reconcile => cmds.push(s(&["up", "-d", "--force-recreate", "--remove-orphans"])),
        StackOp::Destroy => {
            let mut down = s(&["down", "--remove-orphans"]);
            if remove_volumes {
                down.push("-v".into());
            }
            cmds.push(down);
        }
    }
    cmds
}

/// Keep the LAST `max` bytes of a command transcript (errors are at the end), on a char
/// boundary, with a marker when something was dropped.
pub fn truncate_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…[{} bytes dropped]\n{}", start, &s[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_config_overlays_auths_and_keeps_the_hosts_plugins() {
        let auths = vec![
            RegistryAuth {
                server: "https://registry.gitlab.com".into(),
                username: "deploy".into(),
                secret: "glpat-x".into(),
            },
            RegistryAuth {
                server: "docker.io".into(),
                username: "hub".into(),
                secret: "pw".into(),
            },
        ];
        let existing = r#"{"auths":{"ghcr.io":{"auth":"keep"}},"credHelpers":{"ecr":"ecr-login"},"cliPluginsExtraDirs":["/opt/plugins"]}"#;
        let out = docker_config_json(Some(existing), Some(Path::new("/root/.docker")), &auths);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["auths"]["registry.gitlab.com"]["auth"],
            base64::engine::general_purpose::STANDARD.encode("deploy:glpat-x")
        );
        assert_eq!(
            v["auths"]["https://index.docker.io/v1/"]["auth"],
            base64::engine::general_purpose::STANDARD.encode("hub:pw")
        );
        assert_eq!(
            v["auths"]["ghcr.io"]["auth"], "keep",
            "the host's own logins survive"
        );
        assert_eq!(v["credHelpers"]["ecr"], "ecr-login");
        assert_eq!(
            v["cliPluginsExtraDirs"],
            serde_json::json!(["/opt/plugins", "/root/.docker/cli-plugins"])
        );
        // No config on the host at all, and garbage in it, both still produce our auths.
        let bare: serde_json::Value =
            serde_json::from_str(&docker_config_json(None, None, &auths)).unwrap();
        assert!(bare["auths"]["registry.gitlab.com"].is_object());
        assert!(bare.get("cliPluginsExtraDirs").is_none());
        let junk: serde_json::Value =
            serde_json::from_str(&docker_config_json(Some("not json"), None, &auths)).unwrap();
        assert!(junk["auths"]["registry.gitlab.com"].is_object());
    }

    #[test]
    fn injects_label_into_map_list_and_absent_forms() {
        let yaml = r#"
services:
  web:
    image: nginx
    labels:
      traefik.enable: "true"
  worker:
    image: busybox
    labels:
      - "a=b"
      - "io.photon.stack=stale"
  bare:
    image: alpine
"#;
        let out = inject_stack_label(yaml, "stk_42").unwrap();
        let doc: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        let svc = |n: &str| doc["services"][n].clone();
        assert_eq!(
            svc("web")["labels"]["io.photon.stack"].as_str(),
            Some("stk_42")
        );
        assert_eq!(
            svc("web")["labels"]["traefik.enable"].as_str(),
            Some("true")
        );
        let worker: Vec<String> = svc("worker")["labels"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            worker,
            vec!["a=b".to_string(), "io.photon.stack=stk_42".to_string()]
        );
        assert_eq!(
            svc("bare")["labels"]["io.photon.stack"].as_str(),
            Some("stk_42")
        );
        assert_eq!(svc("bare")["image"].as_str(), Some("alpine"));
        // Not a template engine: an interpolation survives untouched.
        let out = inject_stack_label("services:\n  a:\n    image: ${IMG}\n", "x").unwrap();
        assert!(out.contains("${IMG}"));
        assert!(inject_stack_label("not: compose", "x").is_err());
        assert!(inject_stack_label("services: [\n", "x").is_err());
    }

    #[test]
    fn merge_keys_are_applied_before_writing() {
        let yaml = "x-common: &common\n  restart: always\nservices:\n  a:\n    <<: *common\n    image: i\n";
        let out = inject_stack_label(yaml, "s").unwrap();
        let doc: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(doc["services"]["a"]["restart"].as_str(), Some("always"));
        assert!(doc["services"]["a"].get("<<").is_none());
    }

    #[test]
    fn stack_paths_stay_inside_the_folder() {
        assert_eq!(
            safe_relative("compose.yaml").unwrap(),
            PathBuf::from("compose.yaml")
        );
        assert_eq!(
            safe_relative("./conf/app.toml").unwrap(),
            PathBuf::from("conf/app.toml")
        );
        assert!(safe_relative("../etc/passwd").is_err());
        assert!(safe_relative("conf/../../x").is_err());
        assert!(safe_relative("/etc/passwd").is_err());
        assert!(safe_relative("").is_err());
        assert!(safe_relative(".").is_err());
        assert!(valid_project_name("web-app_1"));
        assert!(!valid_project_name("../x"));
        assert!(!valid_project_name("Web"));
        assert!(!valid_project_name("-x"));
        assert!(!valid_project_name(""));
    }

    #[test]
    fn picks_dockers_compose_filename_first() {
        let f = |p: &str| StackFile {
            path: p.into(),
            content: String::new(),
        };
        let files = vec![
            f(".env"),
            f("extra.yml"),
            f("docker-compose.yml"),
            f("compose.yaml"),
        ];
        assert_eq!(compose_file(&files).unwrap().path, "compose.yaml");
        let files = vec![f(".env"), f("sub/compose.yaml"), f("stack.yml")];
        assert_eq!(compose_file(&files).unwrap().path, "stack.yml");
        assert!(compose_file(&[f(".env")]).is_none());
    }

    #[test]
    fn ops_map_to_compose_arguments() {
        let opts = DeployOptions {
            pull: PullPolicy::Always,
            force_recreate: true,
            recreate_volumes: true,
        };
        let up = compose_commands(StackOp::Up, &opts, false, true);
        assert_eq!(up[0], vec!["pull", "--ignore-buildable"]);
        assert_eq!(
            up[1],
            vec!["down", "--remove-orphans", "--volumes"],
            "recreate volumes removes the NAMED volumes, which `up -V` never did"
        );
        assert_eq!(
            up[2],
            vec![
                "up",
                "-d",
                "--remove-orphans",
                "--pull",
                "always",
                "--force-recreate"
            ]
        );
        let up = compose_commands(StackOp::Up, &DeployOptions::default(), false, false);
        assert_eq!(
            up,
            vec![vec!["up", "-d", "--remove-orphans", "--pull", "missing"]]
        );
        // Prepull with `never` would contradict itself, so it is skipped.
        let never = DeployOptions {
            pull: PullPolicy::Never,
            ..Default::default()
        };
        assert_eq!(compose_commands(StackOp::Up, &never, false, true).len(), 1);
        assert_eq!(
            compose_commands(StackOp::Down, &opts, true, false),
            vec![vec!["down"]]
        );
        assert_eq!(
            compose_commands(StackOp::Restart, &opts, false, false),
            vec![vec!["restart"]]
        );
        let re = compose_commands(StackOp::Redeploy, &opts, false, false);
        assert_eq!(re[0][0], "pull");
        assert_eq!(
            re[1],
            vec!["up", "-d", "--force-recreate", "--remove-orphans"]
        );
        assert_eq!(
            compose_commands(StackOp::Reconcile, &opts, false, false),
            vec![vec!["up", "-d", "--force-recreate", "--remove-orphans"]]
        );
        assert_eq!(
            compose_commands(StackOp::Destroy, &opts, true, false),
            vec![vec!["down", "--remove-orphans", "-v"]]
        );
        assert_eq!(
            compose_commands(StackOp::Destroy, &opts, false, false),
            vec![vec!["down", "--remove-orphans"]]
        );
    }

    #[test]
    fn truncates_keeping_the_tail() {
        assert_eq!(truncate_tail("short", 10), "short");
        let long = "a".repeat(20) + "END";
        let t = truncate_tail(&long, 5);
        assert!(t.ends_with("aaEND"));
        assert!(t.starts_with("…[18 bytes dropped]"));
    }
}
