//! Rollout planning (R6): a selector over hosts / a Site / labels resolves to targets, and the
//! targets split into a canary stage and the rest. A port of `frontend/src/lib/deploy/rollouts.ts`
//! so the wizard's preview and the server's plan cannot disagree.

use crate::model::{AgentCapability, AgentStatus, ManagedHost, RolloutStrategy, TargetSelector};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTarget {
    pub host: String,
    pub site: Option<String>,
    pub agent_status: AgentStatus,
    pub eligible: bool,
    /// Why not eligible, or a note ("offline — waits as pending").
    pub note: Option<String>,
}

/// Which managed hosts a selector names, and whether each can take the version at all.
pub fn resolve_targets(selector: &TargetSelector, hosts: &[ManagedHost]) -> Vec<ResolvedTarget> {
    let mut out: Vec<ResolvedTarget> = hosts
        .iter()
        .filter(|h| selector.matches(&h.host, h.site.as_deref(), &h.labels))
        .map(|h| {
            if !h.capabilities.contains(&AgentCapability::Apply) {
                return ResolvedTarget {
                    host: h.host.clone(),
                    site: h.site.clone(),
                    agent_status: h.status,
                    eligible: false,
                    note: Some("agent installed without --docker-apply".into()),
                };
            }
            let note = match h.status {
                AgentStatus::Online => None,
                AgentStatus::Offline => Some("offline — waits as pending".to_string()),
                AgentStatus::Pending => Some("never synced — waits as pending".to_string()),
            };
            ResolvedTarget {
                host: h.host.clone(),
                site: h.site.clone(),
                agent_status: h.status,
                eligible: true,
                note,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.site
            .as_deref()
            .unwrap_or("")
            .cmp(b.site.as_deref().unwrap_or(""))
            .then_with(|| a.host.cmp(&b.host))
    });
    out
}

/// How many eligible targets the canary stage takes: at least one, never all-but-none.
pub fn canary_size(eligible: usize, strategy: &RolloutStrategy) -> usize {
    if eligible == 0 {
        return 0;
    }
    let n = if strategy.canary_is_percent {
        ((eligible as f64 * strategy.canary as f64) / 100.0).ceil() as usize
    } else {
        strategy.canary as usize
    };
    n.clamp(1, eligible)
}

/// Stage per target: 0 = skipped at planning, 1 = canary, 2 = the rest. Online hosts are
/// preferred for the canary so the gate can actually be judged.
pub fn plan_stages(targets: &[ResolvedTarget], strategy: &RolloutStrategy) -> Vec<(String, u32)> {
    let eligible: Vec<&ResolvedTarget> = targets.iter().filter(|t| t.eligible).collect();
    let n = canary_size(eligible.len(), strategy);
    let mut ordered = eligible.clone();
    ordered.sort_by_key(|t| {
        if t.agent_status == AgentStatus::Online {
            0
        } else {
            1
        }
    });
    let canary: Vec<&str> = ordered.iter().take(n).map(|t| t.host.as_str()).collect();
    targets
        .iter()
        .map(|t| {
            let stage = if !t.eligible {
                0
            } else if canary.contains(&t.host.as_str()) {
                1
            } else if eligible.len() > n {
                2
            } else {
                1
            };
            (t.host.clone(), stage)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn host(name: &str, site: Option<&str>, status: AgentStatus, apply: bool) -> ManagedHost {
        ManagedHost {
            id: name.into(),
            host: name.into(),
            site: site.map(|s| s.into()),
            status,
            agent_version: None,
            latest_agent_version: None,
            capabilities: if apply {
                vec![AgentCapability::Read, AgentCapability::Apply]
            } else {
                vec![AgentCapability::Read]
            },
            poll_secs: 15,
            last_seen_ms: None,
            enrolled_ms: 0,
            docker: None,
            labels: BTreeMap::from([("tier".to_string(), "edge".to_string())]),
        }
    }

    #[test]
    fn site_selector_and_eligibility() {
        let hosts = vec![
            host("b", Some("divtik"), AgentStatus::Offline, true),
            host("a", Some("divtik"), AgentStatus::Online, true),
            host("c", Some("divtik"), AgentStatus::Online, false),
            host("d", None, AgentStatus::Online, true),
        ];
        let r = resolve_targets(
            &TargetSelector::Site {
                sites: vec!["divtik".into()],
            },
            &hosts,
        );
        assert_eq!(
            r.iter().map(|t| t.host.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert!(r[0].eligible && r[0].note.is_none());
        assert!(r[1].eligible && r[1].note.as_deref() == Some("offline — waits as pending"));
        assert!(!r[2].eligible);
        let strategy = RolloutStrategy::default();
        let plan = plan_stages(&r, &strategy);
        assert_eq!(
            plan,
            vec![
                ("a".to_string(), 1),
                ("b".to_string(), 2),
                ("c".to_string(), 0)
            ]
        );
    }

    #[test]
    fn label_selector_and_percent_canary() {
        let hosts: Vec<ManagedHost> = (0..10)
            .map(|i| host(&format!("h{i}"), None, AgentStatus::Online, true))
            .collect();
        let r = resolve_targets(
            &TargetSelector::Labels {
                labels: BTreeMap::from([("tier".into(), "edge".into())]),
            },
            &hosts,
        );
        assert_eq!(r.len(), 10);
        let s = RolloutStrategy {
            canary: 25,
            canary_is_percent: true,
            ..Default::default()
        };
        assert_eq!(canary_size(10, &s), 3);
        let plan = plan_stages(&r, &s);
        assert_eq!(plan.iter().filter(|(_, st)| *st == 1).count(), 3);
        assert_eq!(plan.iter().filter(|(_, st)| *st == 2).count(), 7);
        // A single eligible host is the whole rollout: one stage.
        let one = resolve_targets(
            &TargetSelector::Hosts {
                hosts: vec!["h1".into()],
            },
            &hosts,
        );
        assert_eq!(plan_stages(&one, &s), vec![("h1".to_string(), 1)]);
        assert_eq!(canary_size(0, &s), 0);
    }
}
