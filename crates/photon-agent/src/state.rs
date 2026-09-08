//! The agent's persisted local state, `{data_dir}/state.json`. Small on purpose: the desired
//! generation the agent already applied (so the steady-state sync answers `desired: null`),
//! which stack ids it applied at which generation (so a desired document only re-applies the
//! stacks that changed), and the action ids it has already executed (so a lost response never
//! runs a `restart` twice — section 7.4's seen-set). Loaded once at start, saved after every
//! change, and the reason the agent keeps working from what it last knew when the server is
//! unreachable (R1.7).
//!
//! The data dir itself is resolved here too: `/var/lib/photon-agent` by default, created 0700,
//! falling back to `$HOME/.photon-agent` when that is not writable (a dev running the agent as
//! themselves).
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const STATE_FILE: &str = "state.json";
/// How many executed action ids to remember. Actions expire server-side within minutes, so a
/// thousand covers any realistic replay window.
pub const SEEN_ACTIONS_CAP: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedStack {
    pub name: String,
    pub generation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LocalState {
    #[serde(default)]
    pub known_generation: u64,
    /// stack id → what was applied.
    #[serde(default)]
    pub stacks: BTreeMap<String, AppliedStack>,
    /// Executed action ids, oldest first, capped at `SEEN_ACTIONS_CAP`.
    #[serde(default)]
    pub seen_actions: VecDeque<String>,
}

impl LocalState {
    pub fn load(path: &Path) -> LocalState {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "photon-agent: {} is corrupt ({e}); starting from empty state",
                        path.display()
                    );
                    LocalState::default()
                }
            },
            Err(_) => LocalState::default(),
        }
    }

    /// Atomic: write a sibling temp file, then rename over the old one, so a crash mid-write
    /// leaves the previous state rather than half a JSON document.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }

    pub fn has_seen(&self, action_id: &str) -> bool {
        self.seen_actions.iter().any(|a| a == action_id)
    }

    /// Record an action id. Returns `false` when it was already there (do not execute again).
    pub fn mark_seen(&mut self, action_id: &str) -> bool {
        if self.has_seen(action_id) {
            return false;
        }
        self.seen_actions.push_back(action_id.to_string());
        while self.seen_actions.len() > SEEN_ACTIONS_CAP {
            self.seen_actions.pop_front();
        }
        true
    }

    /// Whether a desired stack at `generation` is newer than what this agent applied.
    pub fn stack_needs_apply(&self, stack_id: &str, generation: u64) -> bool {
        match self.stacks.get(stack_id) {
            Some(applied) => generation > applied.generation,
            None => true,
        }
    }

    pub fn record_stack(&mut self, stack_id: &str, name: &str, generation: u64) {
        self.stacks.insert(
            stack_id.to_string(),
            AppliedStack {
                name: name.to_string(),
                generation,
            },
        );
    }

    pub fn forget_stack(&mut self, stack_id: &str) {
        self.stacks.remove(stack_id);
    }

    /// The generation the agent recorded for a compose project it applied, by project name.
    pub fn applied_generation_for(&self, project_name: &str) -> Option<u64> {
        self.stacks
            .values()
            .find(|s| s.name == project_name)
            .map(|s| s.generation)
    }
}

/// Pick the data dir: the configured/default one if it can be created and written to, else
/// `$HOME/.photon-agent`. Created with mode 0700 — stack folders under it hold `.env` files.
pub fn resolve_data_dir(configured: Option<&Path>) -> PathBuf {
    let primary = configured
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(crate::config::DEFAULT_DATA_DIR));
    if ensure_writable_dir(&primary) {
        return primary;
    }
    let home = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| ".".into());
    let fallback = PathBuf::from(home).join(".photon-agent");
    if ensure_writable_dir(&fallback) {
        eprintln!(
            "photon-agent: {} is not writable, using {}",
            primary.display(),
            fallback.display()
        );
        return fallback;
    }
    eprintln!(
        "photon-agent: neither {} nor {} is writable; state will not persist",
        primary.display(),
        fallback.display()
    );
    primary
}

fn ensure_writable_dir(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let probe = dir.join(".write-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("photon-agent-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STATE_FILE);
        let mut s = LocalState {
            known_generation: 7,
            ..Default::default()
        };
        s.record_stack("stk_1", "web", 3);
        assert!(s.mark_seen("act_a"));
        assert!(!s.mark_seen("act_a"));
        s.save(&path).unwrap();
        let back = LocalState::load(&path);
        assert_eq!(back, s);
        assert!(back.has_seen("act_a"));
        assert_eq!(back.applied_generation_for("web"), Some(3));
        assert!(!back.stack_needs_apply("stk_1", 3));
        assert!(back.stack_needs_apply("stk_1", 4));
        assert!(back.stack_needs_apply("stk_2", 1));
        // A missing file is an empty state, not an error; so is a corrupt one.
        assert_eq!(
            LocalState::load(&dir.join("missing.json")),
            LocalState::default()
        );
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(LocalState::load(&path), LocalState::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seen_set_is_capped_oldest_first() {
        let mut s = LocalState::default();
        for i in 0..(SEEN_ACTIONS_CAP + 5) {
            assert!(s.mark_seen(&format!("a{i}")));
        }
        assert_eq!(s.seen_actions.len(), SEEN_ACTIONS_CAP);
        assert!(!s.has_seen("a0"));
        assert!(!s.has_seen("a4"));
        assert!(s.has_seen("a5"));
        assert!(s.has_seen(&format!("a{}", SEEN_ACTIONS_CAP + 4)));
    }

    #[test]
    fn data_dir_falls_back_to_home_when_primary_unwritable() {
        let base = std::env::temp_dir().join(format!("photon-agent-dd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // A regular FILE where the dir should be makes create_dir_all fail.
        let blocked = base.join("blocked");
        std::fs::write(&blocked, b"x").unwrap();
        let home_before = std::env::var("HOME").ok();
        std::env::set_var("HOME", &base);
        let got = resolve_data_dir(Some(&blocked));
        match home_before {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        assert_eq!(got, base.join(".photon-agent"));
        assert!(got.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }
}
