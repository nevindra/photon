//! `photon-agent install` / `uninstall`: the systemd unit and its env file. The install snippet
//! the UI shows runs exactly this; the SSH bootstrap runs it too. Uninstall is documented and
//! clean (R9.5): the unit and the env file go, the containers Photon started keep running.
use std::path::Path;

use crate::config::AgentConfig;
use crate::prompt;

pub const BIN_PATH: &str = "/usr/local/bin/photon-agent";
pub const ENV_DIR: &str = "/etc/photon-agent";
pub const ENV_PATH: &str = "/etc/photon-agent/agent.env";
pub const UNIT_PATH: &str = "/etc/systemd/system/photon-agent.service";
pub const SERVICE: &str = "photon-agent";

pub fn unit_text() -> String {
    format!(
        "[Unit]\n\
Description=Photon host agent (resource metrics + Docker fleet management)\n\
Documentation=https://github.com/nodeflux/photon\n\
After=network-online.target docker.service\n\
Wants=network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
EnvironmentFile=-{ENV_PATH}\n\
ExecStart={BIN_PATH}\n\
Restart=always\n\
RestartSec=5\n\
KillMode=process\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n"
    )
}

fn has_systemd() -> bool {
    Path::new("/run/systemd/system").is_dir() && which("systemctl")
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Install: env file (0600), binary in place, unit, enable + start.
///
/// Root is checked FIRST, and a missing token is prompted for (echo off) rather than demanded
/// on the command line — see `prompt.rs` for why the snippet leaves it out.
pub fn install(cfg: &AgentConfig) -> Result<(), String> {
    if cfg.server.is_none() {
        return Err("install needs --server <url> (the Photon this host should dial)".into());
    }
    if !prompt::is_root() {
        return Err(format!(
            "install must run as root: it writes {BIN_PATH} and {ENV_PATH} and enables a systemd \
             unit. Re-run it with sudo (the token prompt still works under sudo)."
        ));
    }
    let token = prompt::resolve_token(cfg.token.clone(), "install")?;
    let cfg = AgentConfig {
        token: Some(token),
        ..cfg.clone()
    };
    let cfg = &cfg;
    std::fs::create_dir_all(ENV_DIR).map_err(|e| format!("creating {ENV_DIR}: {e}"))?;
    let mut env = cfg.env_lines().join("\n");
    env.push('\n');
    std::fs::write(ENV_PATH, env).map_err(|e| format!("writing {ENV_PATH}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(ENV_PATH, std::fs::Permissions::from_mode(0o600));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = Path::new(BIN_PATH);
    let same = std::fs::canonicalize(&exe).ok() == std::fs::canonicalize(target).ok();
    if !same {
        std::fs::copy(&exe, target).map_err(|e| format!("copying to {BIN_PATH}: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755));
        }
        println!("installed binary to {BIN_PATH}");
    }
    println!("wrote {ENV_PATH}");
    if !has_systemd() {
        println!("systemd not found — write this unit yourself and start {BIN_PATH} with {ENV_PATH}:\n\n{}", unit_text());
        return Ok(());
    }
    std::fs::write(UNIT_PATH, unit_text()).map_err(|e| format!("writing {UNIT_PATH}: {e}"))?;
    run("systemctl", &["daemon-reload"])?;
    run("systemctl", &["enable", "--now", SERVICE])?;
    // A previous instance keeps the old env until restarted.
    let _ = run("systemctl", &["restart", SERVICE]);
    println!("enabled and started {SERVICE}.service — the host appears in Photon once its first sync lands");
    Ok(())
}

/// Uninstall: stop + disable + remove the unit and env. Containers keep running.
pub fn uninstall() -> Result<(), String> {
    if has_systemd() {
        let _ = run("systemctl", &["disable", "--now", SERVICE]);
        if Path::new(UNIT_PATH).exists() {
            std::fs::remove_file(UNIT_PATH).map_err(|e| e.to_string())?;
        }
        let _ = run("systemctl", &["daemon-reload"]);
    }
    if Path::new(ENV_PATH).exists() {
        std::fs::remove_file(ENV_PATH).map_err(|e| e.to_string())?;
    }
    println!("removed {SERVICE}.service and {ENV_PATH}. Containers and stacks keep running; remove the stack folders under the data dir yourself if you want them gone.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_points_at_env_and_binary() {
        let u = unit_text();
        assert!(u.contains("EnvironmentFile=-/etc/photon-agent/agent.env"));
        assert!(u.contains("ExecStart=/usr/local/bin/photon-agent"));
        assert!(u.contains("After=network-online.target docker.service"));
    }
}
