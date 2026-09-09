//! Self-upgrade over the sync channel (R1.10): the server advertises a version and a URL; the
//! agent downloads to its data dir, verifies the digest when one was given, swaps the binary
//! over its own path and re-executes itself with the same arguments. SSH is for the first
//! install only; every upgrade after that rides this path, which works behind NAT.
use std::path::Path;

use photon_deploy::proto::UpgradeSpec;
use sha2::{Digest, Sha256};

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Download, verify, swap, and exec. Returns only on failure (a success never returns).
pub async fn apply(
    client: &reqwest::Client,
    token: &str,
    spec: &UpgradeSpec,
    data_dir: &Path,
) -> Result<(), String> {
    if spec.version == CURRENT_VERSION {
        return Ok(());
    }
    eprintln!(
        "photon-agent: upgrading {CURRENT_VERSION} → {} from {}",
        spec.version, spec.url
    );
    let resp = client
        .get(&spec.url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download answered {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if bytes.len() < 1024 {
        return Err("downloaded binary is implausibly small".into());
    }
    if let Some(want) = spec.sha256.as_deref().filter(|s| !s.is_empty()) {
        let got = hex_digest(&bytes);
        if !got.eq_ignore_ascii_case(want) {
            return Err(format!("sha256 mismatch: got {got}, expected {want}"));
        }
    }
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let staged = data_dir.join("photon-agent.new");
    std::fs::write(&staged, &bytes).map_err(|e| format!("writing {}: {e}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    // Same filesystem as the running binary? rename is atomic only then; fall back to copy.
    if std::fs::rename(&staged, &exe).is_err() {
        std::fs::copy(&staged, &exe).map_err(|e| format!("replacing {}: {e}", exe.display()))?;
        let _ = std::fs::remove_file(&staged);
    }
    eprintln!("photon-agent: installed {}, restarting", spec.version);
    reexec(&exe)
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(unix)]
fn reexec(exe: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let err = std::process::Command::new(exe).args(args).exec();
    Err(format!("exec failed: {err}"))
}

#[cfg(not(unix))]
fn reexec(_exe: &Path) -> Result<(), String> {
    Err("self-restart is only supported on unix; restart the service by hand".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_lowercase_hex() {
        assert_eq!(hex_digest(b"").len(), 64);
        assert!(hex_digest(b"x").chars().all(|c| c.is_ascii_hexdigit()));
    }
}
