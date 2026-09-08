pub mod bootstrap;
pub mod cipher;
pub mod store;

/// The version of the agent binary this server can hand out — read by running the bundled
/// binary's `version` subcommand once at startup. Looks in the configured binaries directory
/// for this machine's arch first, then for a `photon-agent` beside `photon-server` (how the
/// container image ships it). `None` when there is no binary: the UI then shows no upgrade
/// badge, and `POST /api/agents/:host/upgrade` refuses rather than promising a download that
/// would 404. Deliberately NOT `photon-server`'s own version — the crates are versioned apart.
pub fn bundled_agent_version(binaries_dir: &std::path::Path) -> Option<String> {
    let arch = std::env::consts::ARCH;
    let mut candidates = vec![
        binaries_dir.join(format!("photon-agent-{arch}")),
        binaries_dir.join(format!("photon-agent-{arch}-unknown-linux-gnu")),
        binaries_dir.join(format!("photon-agent-{arch}-unknown-linux-musl")),
    ];
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe.with_file_name("photon-agent"));
    }
    let bin = candidates.into_iter().find(|p| p.is_file())?;
    let out = std::process::Command::new(&bin)
        .arg("version")
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "photon-server: {} version exited {}; no agent upgrade will be offered",
            bin.display(),
            out.status
        );
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let v = text.split_whitespace().last()?.trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}
