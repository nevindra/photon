//! The persistence boundary. photon-server implements `DeployStore` over the control-plane
//! SQLite; `MemStore` here is the in-memory implementation every test in the workspace uses.
//!
//! Rows are stored whole (the store is a document store with a few indexed columns): volumes are
//! small — hundreds of hosts, thousands of stacks — and the business rules live in photon-api
//! and photon-server's supervisor, not in SQL.

use crate::model::*;
use async_trait::async_trait;
use photon_core::PhotonError;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// One host's latest observed inventory, as reported on its last sync.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObservedRecord {
    pub host: String,
    pub reported_ms: i64,
    pub observed: Observed,
}

/// The stored secret values for a stack, encrypted by photon-server before they get here and
/// decrypted after — the store only ever sees ciphertext.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct SecretBlob {
    pub stack_id: String,
    /// `name → base64(nonce || ciphertext)`.
    pub values: BTreeMap<String, String>,
}

/// A stored registry credential: the public row plus its secret as ciphertext (the cipher is
/// photon-server's; the store never sees the plaintext, same as `SecretBlob`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegistryCredentialRecord {
    pub credential: RegistryCredential,
    /// `base64(nonce || ciphertext)` of the token / password.
    pub secret_blob: String,
}

/// One edit of a host row, run under the store's lock by `DeployStore::update_host`.
pub type HostEdit = Box<dyn for<'r> FnOnce(&'r mut HostRecord) + Send + 'static>;

#[async_trait]
pub trait DeployStore: Send + Sync {
    // --- managed hosts ---
    async fn list_hosts(&self) -> Result<Vec<HostRecord>, PhotonError>;
    async fn get_host(&self, host: &str) -> Result<Option<HostRecord>, PhotonError>;
    async fn get_host_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<HostRecord>, PhotonError>;
    /// Insert or replace by `host` name. Creation only — a live record is changed through
    /// `update_host`, never read-modify-written from the caller's copy: the sync fold and a UI
    /// edit race on the same row, and whoever writes second would erase the other's change
    /// (the observed case: a stack's destroy bumped the host generation between a sync's read
    /// and its write, the write put the old generation back, and the agent was never handed
    /// the destroy). `desired_generation` is never lowered by a put, so even a stale put cannot
    /// hide a bump.
    async fn put_host(&self, rec: &HostRecord) -> Result<(), PhotonError>;
    /// Atomic read-modify-write of one host under the store's lock: `f` sees the current row
    /// and changes only what it owns. `Ok(false)` when there is no such host.
    async fn update_host(&self, host: &str, f: HostEdit) -> Result<bool, PhotonError>;
    async fn delete_host(&self, host: &str) -> Result<bool, PhotonError>;

    // --- observed inventory ---
    async fn put_observed(&self, rec: &ObservedRecord) -> Result<(), PhotonError>;
    async fn get_observed(&self, host: &str) -> Result<Option<ObservedRecord>, PhotonError>;

    // --- one-shot actions ---
    async fn put_action(&self, a: &ActionRecord) -> Result<(), PhotonError>;
    async fn get_action(&self, id: &str) -> Result<Option<ActionRecord>, PhotonError>;
    /// Newest first.
    async fn list_actions(
        &self,
        host: &str,
        limit: usize,
    ) -> Result<Vec<ActionRecord>, PhotonError>;
    /// Queued or running actions, oldest first — what the sync response carries.
    async fn open_actions(&self, host: &str) -> Result<Vec<ActionRecord>, PhotonError>;

    // --- stacks ---
    async fn list_stacks(&self) -> Result<Vec<Stack>, PhotonError>;
    async fn get_stack(&self, id: &str) -> Result<Option<Stack>, PhotonError>;
    async fn put_stack(&self, s: &Stack) -> Result<(), PhotonError>;
    async fn delete_stack(&self, id: &str) -> Result<bool, PhotonError>;
    async fn put_secrets(&self, blob: &SecretBlob) -> Result<(), PhotonError>;
    async fn get_secrets(&self, stack_id: &str) -> Result<Option<SecretBlob>, PhotonError>;

    // --- templates ---
    async fn list_templates(&self) -> Result<Vec<Template>, PhotonError>;
    async fn get_template(&self, id: &str) -> Result<Option<Template>, PhotonError>;
    async fn put_template(&self, t: &Template) -> Result<(), PhotonError>;
    async fn delete_template(&self, id: &str) -> Result<bool, PhotonError>;
    async fn get_version(&self, id: &str) -> Result<Option<TemplateVersion>, PhotonError>;
    async fn list_versions(&self, template_id: &str) -> Result<Vec<TemplateVersion>, PhotonError>;
    /// Versions are immutable (R5.4): this is insert-only and refuses an existing id.
    async fn insert_version(&self, v: &TemplateVersion) -> Result<(), PhotonError>;

    // --- registries ---
    async fn list_registries(&self) -> Result<Vec<Registry>, PhotonError>;
    async fn put_registry(&self, r: &Registry) -> Result<(), PhotonError>;
    async fn delete_registry(&self, id: &str) -> Result<bool, PhotonError>;

    // --- image registry credentials ---
    async fn list_registry_credentials(&self)
        -> Result<Vec<RegistryCredentialRecord>, PhotonError>;
    async fn put_registry_credential(
        &self,
        r: &RegistryCredentialRecord,
    ) -> Result<(), PhotonError>;
    async fn delete_registry_credential(&self, id: &str) -> Result<bool, PhotonError>;

    // --- rollouts ---
    async fn list_rollouts(&self) -> Result<Vec<Rollout>, PhotonError>;
    async fn get_rollout(&self, id: &str) -> Result<Option<Rollout>, PhotonError>;
    async fn put_rollout(&self, r: &Rollout) -> Result<(), PhotonError>;

    // --- audit ---
    async fn append_audit(&self, e: &AuditEntry) -> Result<(), PhotonError>;
    /// Newest first, filtered; `total` is the count before `limit`.
    async fn query_audit(&self, f: &AuditFilter) -> Result<(Vec<AuditEntry>, usize), PhotonError>;
}

/// Apply an `AuditFilter` to one entry — shared by `MemStore` and the SQLite store's post-filter
/// so both answer the same question the same way.
pub fn audit_matches(e: &AuditEntry, f: &AuditFilter) -> bool {
    if let Some(since) = f.since_ms {
        if e.ts_ms < since {
            return false;
        }
    }
    if let Some(a) = &f.actor {
        if !a.is_empty() && &e.actor != a {
            return false;
        }
    }
    if let Some(a) = &f.action {
        if !a.is_empty() && e.action != *a && !e.action.starts_with(&format!("{a}.")) {
            return false;
        }
    }
    if let Some(h) = &f.host {
        if !h.is_empty() && e.target.host.as_deref() != Some(h.as_str()) {
            return false;
        }
    }
    if let Some(r) = f.result {
        if e.result != r {
            return false;
        }
    }
    if let Some(q) = &f.q {
        let q = q.trim().to_lowercase();
        if !q.is_empty() {
            let hay = format!(
                "{} {} {} {} {}",
                e.actor,
                e.action,
                e.detail.as_deref().unwrap_or(""),
                serde_json::to_string(&e.target).unwrap_or_default(),
                e.result.as_str()
            )
            .to_lowercase();
            if !hay.contains(&q) {
                return false;
            }
        }
    }
    true
}

/// In-memory `DeployStore` for tests and for the API crate's unit tests.
#[derive(Default)]
pub struct MemStore {
    inner: Mutex<MemInner>,
}

#[derive(Default)]
struct MemInner {
    hosts: BTreeMap<String, HostRecord>,
    observed: BTreeMap<String, ObservedRecord>,
    actions: Vec<ActionRecord>,
    stacks: BTreeMap<String, Stack>,
    secrets: BTreeMap<String, SecretBlob>,
    templates: BTreeMap<String, Template>,
    versions: BTreeMap<String, TemplateVersion>,
    registries: BTreeMap<String, Registry>,
    registry_credentials: BTreeMap<String, RegistryCredentialRecord>,
    rollouts: BTreeMap<String, Rollout>,
    audit: Vec<AuditEntry>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn dup(what: &str, id: &str) -> PhotonError {
    PhotonError::Io(format!("{what} {id} already exists"))
}

#[async_trait]
impl DeployStore for MemStore {
    async fn list_hosts(&self) -> Result<Vec<HostRecord>, PhotonError> {
        Ok(self.inner.lock().unwrap().hosts.values().cloned().collect())
    }
    async fn get_host(&self, host: &str) -> Result<Option<HostRecord>, PhotonError> {
        Ok(self.inner.lock().unwrap().hosts.get(host).cloned())
    }
    async fn get_host_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<HostRecord>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .hosts
            .values()
            .find(|h| h.token_hash == token_hash)
            .cloned())
    }
    async fn put_host(&self, rec: &HostRecord) -> Result<(), PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let mut rec = rec.clone();
        if let Some(cur) = g.hosts.get(&rec.host) {
            rec.desired_generation = rec.desired_generation.max(cur.desired_generation);
        }
        g.hosts.insert(rec.host.clone(), rec);
        Ok(())
    }
    async fn update_host(&self, host: &str, f: HostEdit) -> Result<bool, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        match g.hosts.get_mut(host) {
            Some(r) => {
                f(r);
                Ok(true)
            }
            None => Ok(false),
        }
    }
    async fn delete_host(&self, host: &str) -> Result<bool, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        g.observed.remove(host);
        Ok(g.hosts.remove(host).is_some())
    }

    async fn put_observed(&self, rec: &ObservedRecord) -> Result<(), PhotonError> {
        self.inner
            .lock()
            .unwrap()
            .observed
            .insert(rec.host.clone(), rec.clone());
        Ok(())
    }
    async fn get_observed(&self, host: &str) -> Result<Option<ObservedRecord>, PhotonError> {
        Ok(self.inner.lock().unwrap().observed.get(host).cloned())
    }

    async fn put_action(&self, a: &ActionRecord) -> Result<(), PhotonError> {
        let mut g = self.inner.lock().unwrap();
        if let Some(x) = g.actions.iter_mut().find(|x| x.id == a.id) {
            *x = a.clone();
        } else {
            g.actions.push(a.clone());
        }
        Ok(())
    }
    async fn get_action(&self, id: &str) -> Result<Option<ActionRecord>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .actions
            .iter()
            .find(|a| a.id == id)
            .cloned())
    }
    async fn list_actions(
        &self,
        host: &str,
        limit: usize,
    ) -> Result<Vec<ActionRecord>, PhotonError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<ActionRecord> = g
            .actions
            .iter()
            .filter(|a| a.host == host)
            .cloned()
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
        v.truncate(limit);
        Ok(v)
    }
    async fn open_actions(&self, host: &str) -> Result<Vec<ActionRecord>, PhotonError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<ActionRecord> = g
            .actions
            .iter()
            .filter(|a| {
                a.host == host && matches!(a.status, ActionStatus::Queued | ActionStatus::Running)
            })
            .cloned()
            .collect();
        v.sort_by_key(|a| a.created_ms);
        Ok(v)
    }

    async fn list_stacks(&self) -> Result<Vec<Stack>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .stacks
            .values()
            .cloned()
            .collect())
    }
    async fn get_stack(&self, id: &str) -> Result<Option<Stack>, PhotonError> {
        Ok(self.inner.lock().unwrap().stacks.get(id).cloned())
    }
    async fn put_stack(&self, s: &Stack) -> Result<(), PhotonError> {
        self.inner
            .lock()
            .unwrap()
            .stacks
            .insert(s.id.clone(), s.clone());
        Ok(())
    }
    async fn delete_stack(&self, id: &str) -> Result<bool, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        g.secrets.remove(id);
        Ok(g.stacks.remove(id).is_some())
    }
    async fn put_secrets(&self, blob: &SecretBlob) -> Result<(), PhotonError> {
        self.inner
            .lock()
            .unwrap()
            .secrets
            .insert(blob.stack_id.clone(), blob.clone());
        Ok(())
    }
    async fn get_secrets(&self, stack_id: &str) -> Result<Option<SecretBlob>, PhotonError> {
        Ok(self.inner.lock().unwrap().secrets.get(stack_id).cloned())
    }

    async fn list_templates(&self) -> Result<Vec<Template>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .templates
            .values()
            .cloned()
            .collect())
    }
    async fn get_template(&self, id: &str) -> Result<Option<Template>, PhotonError> {
        Ok(self.inner.lock().unwrap().templates.get(id).cloned())
    }
    async fn put_template(&self, t: &Template) -> Result<(), PhotonError> {
        self.inner
            .lock()
            .unwrap()
            .templates
            .insert(t.id.clone(), t.clone());
        Ok(())
    }
    async fn delete_template(&self, id: &str) -> Result<bool, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        g.versions.retain(|_, v| v.template_id != id);
        Ok(g.templates.remove(id).is_some())
    }
    async fn get_version(&self, id: &str) -> Result<Option<TemplateVersion>, PhotonError> {
        Ok(self.inner.lock().unwrap().versions.get(id).cloned())
    }
    async fn list_versions(&self, template_id: &str) -> Result<Vec<TemplateVersion>, PhotonError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<TemplateVersion> = g
            .versions
            .values()
            .filter(|v| v.template_id == template_id)
            .cloned()
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
        Ok(v)
    }
    async fn insert_version(&self, v: &TemplateVersion) -> Result<(), PhotonError> {
        let mut g = self.inner.lock().unwrap();
        if g.versions.contains_key(&v.id) {
            return Err(dup("template version", &v.id));
        }
        g.versions.insert(v.id.clone(), v.clone());
        Ok(())
    }

    async fn list_registries(&self) -> Result<Vec<Registry>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .registries
            .values()
            .cloned()
            .collect())
    }
    async fn put_registry(&self, r: &Registry) -> Result<(), PhotonError> {
        self.inner
            .lock()
            .unwrap()
            .registries
            .insert(r.id.clone(), r.clone());
        Ok(())
    }
    async fn delete_registry(&self, id: &str) -> Result<bool, PhotonError> {
        Ok(self.inner.lock().unwrap().registries.remove(id).is_some())
    }

    async fn list_registry_credentials(
        &self,
    ) -> Result<Vec<RegistryCredentialRecord>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .registry_credentials
            .values()
            .cloned()
            .collect())
    }
    async fn put_registry_credential(
        &self,
        r: &RegistryCredentialRecord,
    ) -> Result<(), PhotonError> {
        self.inner
            .lock()
            .unwrap()
            .registry_credentials
            .insert(r.credential.id.clone(), r.clone());
        Ok(())
    }
    async fn delete_registry_credential(&self, id: &str) -> Result<bool, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .registry_credentials
            .remove(id)
            .is_some())
    }

    async fn list_rollouts(&self) -> Result<Vec<Rollout>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .rollouts
            .values()
            .cloned()
            .collect())
    }
    async fn get_rollout(&self, id: &str) -> Result<Option<Rollout>, PhotonError> {
        Ok(self.inner.lock().unwrap().rollouts.get(id).cloned())
    }
    async fn put_rollout(&self, r: &Rollout) -> Result<(), PhotonError> {
        self.inner
            .lock()
            .unwrap()
            .rollouts
            .insert(r.id.clone(), r.clone());
        Ok(())
    }

    async fn append_audit(&self, e: &AuditEntry) -> Result<(), PhotonError> {
        self.inner.lock().unwrap().audit.push(e.clone());
        Ok(())
    }
    async fn query_audit(&self, f: &AuditFilter) -> Result<(Vec<AuditEntry>, usize), PhotonError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<AuditEntry> = g
            .audit
            .iter()
            .filter(|e| audit_matches(e, f))
            .cloned()
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.ts_ms));
        let total = v.len();
        if f.limit > 0 {
            v.truncate(f.limit);
        }
        Ok((v, total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: i64, actor: &str, action: &str, host: &str) -> AuditEntry {
        AuditEntry {
            id: format!("a{ts}"),
            ts_ms: ts,
            actor: actor.into(),
            role: Role::Operator,
            action: action.into(),
            target: AuditTarget {
                host: Some(host.into()),
                ..Default::default()
            },
            result: AuditResultKind::Ok,
            detail: Some("exit 0".into()),
            payload_digest: None,
        }
    }

    #[tokio::test]
    async fn audit_filters_by_family_and_text() {
        let s = MemStore::new();
        s.append_audit(&entry(1, "rina", "container.restart", "web-1"))
            .await
            .unwrap();
        s.append_audit(&entry(2, "rina", "stack.apply", "web-2"))
            .await
            .unwrap();
        s.append_audit(&entry(3, "bob", "stack.destroy", "web-1"))
            .await
            .unwrap();
        let (v, total) = s
            .query_audit(&AuditFilter {
                action: Some("stack".into()),
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(v[0].ts_ms, 3, "newest first");
        let (v, _) = s
            .query_audit(&AuditFilter {
                host: Some("web-1".into()),
                q: Some("RESTART".into()),
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(v.len(), 1);
        let (v, total) = s
            .query_audit(&AuditFilter {
                limit: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!((v.len(), total), (1, 3));
    }

    fn host(name: &str) -> HostRecord {
        HostRecord {
            id: format!("mh_{name}"),
            host: name.into(),
            site: None,
            token_hash: "h".into(),
            requested_capabilities: vec![],
            reported_capabilities: vec![],
            agent_version: None,
            upgrade_to: None,
            poll_secs: 15,
            last_seen_ms: None,
            enrolled_ms: 0,
            docker: None,
            host_id: None,
            arch: None,
            os: None,
            labels: BTreeMap::new(),
            desired_generation: 0,
        }
    }

    /// The race behind a stack that sat in `destroying` forever: a sync fold read the host,
    /// a destroy bumped its generation, the fold wrote its stale copy back. `put_host` never
    /// lowers the generation and `update_host` edits under the lock, so neither order loses
    /// the bump.
    #[tokio::test]
    async fn a_stale_put_cannot_lower_the_host_generation() {
        let s = MemStore::new();
        s.put_host(&host("web-1")).await.unwrap();
        let stale = s.get_host("web-1").await.unwrap().unwrap();
        assert!(s
            .update_host("web-1", Box::new(|r| r.desired_generation += 1))
            .await
            .unwrap());
        let mut stale = stale;
        stale.agent_version = Some("0.2.0".into());
        s.put_host(&stale).await.unwrap();
        let cur = s.get_host("web-1").await.unwrap().unwrap();
        assert_eq!(cur.desired_generation, 1, "the bump survived the stale put");
        assert_eq!(cur.agent_version.as_deref(), Some("0.2.0"));
        assert!(!s
            .update_host("nope", Box::new(|r| r.desired_generation += 1))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn versions_are_insert_only() {
        let s = MemStore::new();
        let v = TemplateVersion {
            id: "tv_1".into(),
            template_id: "t".into(),
            version: "1.0.0".into(),
            created_ms: 0,
            created_by: "x".into(),
            notes: None,
            deployed_count: 0,
            manifest: serde_json::from_str(r#"{"id":"t","version":"1.0.0","name":"T"}"#).unwrap(),
            compose_text: String::new(),
        };
        s.insert_version(&v).await.unwrap();
        assert!(s.insert_version(&v).await.is_err());
    }
}
