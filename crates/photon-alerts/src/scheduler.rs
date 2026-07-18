//! The alert evaluation loop: owns per-(rule,series) state, samples due rules via `ConditionSource`,
//! folds each series through `apply`, opens/closes incidents, and fans deliveries out to channels.
//! Mirrors `photon-uptime::scheduler`. Non-fatal: an eval/query error leaves state unchanged.
use crate::model::*;
use crate::notify::{build_payload, Notifier, NotifyStatus};
use crate::source::ConditionSource;
use crate::state::apply;
use crate::store::AlertStore;
use photon_core::PhotonError;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

/// Fold one full sample of a rule's series into durable state + notifications.
pub async fn process_sample<S: AlertStore, N: Notifier>(
    store: &S,
    notifier: &N,
    rule: &Rule,
    states: &mut HashMap<String, SeriesState>,
    samples: Vec<SeriesSample>,
    now: i64,
) -> Result<(), PhotonError> {
    let threshold = rule.condition.threshold();
    let cmp = rule.condition.cmp();
    let mut seen: Vec<String> = Vec::with_capacity(samples.len());

    for s in &samples {
        let key = s.series_key();
        seen.push(key.clone());
        let prev = *states.get(&key).unwrap_or(&SeriesState::ok());
        let breaching = cmp.test(s.value, threshold);
        let (next, transition) = apply(prev, breaching, s.value, rule.for_secs, now);
        states.insert(key.clone(), next);
        apply_transition(store, notifier, rule, &key, s, transition, now).await?;
        if matches!(next.phase, AlertPhase::Triggered) {
            if let Some(id) = store.open_incident_for(&rule.id, &key).await? {
                store.bump_incident_peak(id, s.value).await?;
            }
        }
    }

    // Series that were Triggered but vanished from a successful sample → resolve them.
    let vanished: Vec<String> = states
        .keys()
        .filter(|k| !seen.contains(k))
        .cloned()
        .collect();
    for key in vanished {
        let prev = states[&key];
        if matches!(prev.phase, AlertPhase::Triggered) {
            // Rebuild the label pairs from the stored `series_key` so the resolve webhook still
            // says *which* series recovered (an empty `key` would emit `"series": {}`).
            let s = SeriesSample {
                key: parse_series_key(&key),
                value: prev.last_value,
            };
            apply_transition(
                store,
                notifier,
                rule,
                &key,
                &s,
                Some(Transition::Resolved),
                now,
            )
            .await?;
        }
        // A genuinely-gone series needs no retained state: drop it instead of re-inserting `Ok`, so
        // a churning high-cardinality `group_by` can't leak `Ok` entries forever. It re-seeds as
        // `Ok` (via `unwrap_or(&SeriesState::ok())` above) if the series ever reappears.
        states.remove(&key);
    }
    Ok(())
}

/// Rebuild the label pairs from a canonical `series_key` string (`k=v,k=v`, sorted; `""` ⇒ `vec![]`)
/// — the inverse of [`SeriesSample::series_key`]. Splits on `,`, then each pair on the *first* `=`
/// so a value containing `=` survives intact.
fn parse_series_key(key: &str) -> Vec<(String, String)> {
    if key.is_empty() {
        return Vec::new();
    }
    key.split(',')
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

async fn apply_transition<S: AlertStore, N: Notifier>(
    store: &S,
    notifier: &N,
    rule: &Rule,
    key: &str,
    s: &SeriesSample,
    transition: Option<Transition>,
    now: i64,
) -> Result<(), PhotonError> {
    let threshold = rule.condition.threshold();
    match transition {
        Some(Transition::Triggered) => {
            let id = store
                .open_incident(
                    &rule.id,
                    key,
                    now,
                    s.value,
                    rule.severity,
                    &rule.condition.summary(),
                )
                .await?;
            fan_out(
                store,
                notifier,
                rule,
                s,
                NotifyStatus::Triggered,
                s.value,
                threshold,
                now,
                now,
                id,
            )
            .await?;
        }
        Some(Transition::Resolved) => {
            let (started, id) = match store.open_incident_for(&rule.id, key).await? {
                Some(id) => {
                    store.close_incident(id, now).await?;
                    (now, id)
                }
                None => (now, 0),
            };
            fan_out(
                store,
                notifier,
                rule,
                s,
                NotifyStatus::Resolved,
                s.value,
                threshold,
                started,
                now,
                id,
            )
            .await?;
        }
        None => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn fan_out<S: AlertStore, N: Notifier>(
    store: &S,
    notifier: &N,
    rule: &Rule,
    s: &SeriesSample,
    status: NotifyStatus,
    value: f64,
    threshold: f64,
    started_at: i64,
    at: i64,
    incident_id: i64,
) -> Result<(), PhotonError> {
    let payload = build_payload(
        rule,
        &s.labels_json(),
        status,
        value,
        threshold,
        started_at,
        at,
        incident_id,
    );
    for cid in &rule.channel_ids {
        match store.get_channel(cid).await? {
            Some(ch) => notifier.deliver(&ch, payload.clone()).await,
            None => eprintln!("alert rule {}: channel {cid} not found, skipping", rule.id),
        }
    }
    Ok(())
}

struct Slot {
    rule: Rule,
    states: HashMap<String, SeriesState>,
    next_due: i64,
}

pub async fn run<S, C, N>(
    store: Arc<S>,
    source: Arc<C>,
    notifier: Arc<N>,
    mut cmd_rx: mpsc::Receiver<SchedulerCommand>,
    concurrency: usize,
) where
    S: AlertStore,
    C: ConditionSource,
    N: Notifier + 'static,
{
    let mut slots: HashMap<RuleId, Slot> = HashMap::new();
    // Seed rules + rebuild Triggered state from open incidents.
    let open = store.list_open_incidents().await.unwrap_or_default();
    if let Ok(rules) = store.list_rules().await {
        for rule in rules {
            let mut states = HashMap::new();
            for inc in open.iter().filter(|i| i.rule_id == rule.id) {
                states.insert(
                    inc.series_key.clone(),
                    SeriesState::triggered_since(inc.started_at),
                );
            }
            slots.insert(
                rule.id.clone(),
                Slot {
                    rule,
                    states,
                    next_due: now_ms(),
                },
            );
        }
    }

    let sem = Arc::new(Semaphore::new(concurrency.max(1)));
    let (done_tx, mut done_rx) = mpsc::channel::<(RuleId, Vec<SeriesSample>)>(1024);
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(1000));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = now_ms();
                for slot in slots.values_mut() {
                    if !slot.rule.enabled || slot.next_due > now { continue; }
                    slot.next_due = now + slot.rule.interval_secs.max(1) * 1000;
                    let (source, sem, done_tx) = (source.clone(), sem.clone(), done_tx.clone());
                    let (rid, cond) = (slot.rule.id.clone(), slot.rule.condition.clone());
                    tokio::spawn(async move {
                        let Ok(_permit) = sem.acquire_owned().await else { return };
                        match source.sample(&cond, now_ms()).await {
                            Ok(samples) => { let _ = done_tx.send((rid, samples)).await; }
                            Err(e) => eprintln!("alert eval {rid}: {e}"), // state unchanged: skip tick
                        }
                    });
                }
            }
            Some((rid, samples)) = done_rx.recv() => {
                if let Some(slot) = slots.get_mut(&rid) {
                    let (rule, states) = (slot.rule.clone(), &mut slot.states);
                    if let Err(e) = process_sample(&*store, &*notifier, &rule, states, samples, now_ms()).await {
                        eprintln!("alert process {rid}: {e}");
                    }
                }
            }
            cmd = cmd_rx.recv() => match cmd {
                Some(SchedulerCommand::Upsert(r)) => {
                    let states = slots.remove(&r.id).map(|s| s.states).unwrap_or_default();
                    slots.insert(r.id.clone(), Slot { rule: *r, states, next_due: now_ms() });
                }
                Some(SchedulerCommand::Remove(id)) => { slots.remove(&id); }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::FakeNotifier;
    use crate::store::mem::MemStore;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn rule(for_secs: i64) -> Rule {
        Rule {
            id: "r1".into(),
            name: "cpu".into(),
            description: None,
            enabled: true,
            condition: Condition::Metrics(MetricCondition {
                metric_name: "m".into(),
                label_filters: Default::default(),
                group_by: vec!["host.name".into()],
                agg: MetricAgg::Avg,
                window_secs: 60,
                cmp: Cmp::Gt,
                threshold: 0.9,
            }),
            for_secs,
            interval_secs: 60,
            severity: Severity::Warning,
            channel_ids: vec!["c1".into()],
            created_at: 0,
            updated_at: 0,
        }
    }
    fn sample(host: &str, v: f64) -> SeriesSample {
        SeriesSample {
            key: vec![("host.name".into(), host.into())],
            value: v,
        }
    }

    #[tokio::test]
    async fn per_series_trigger_and_resolve_notify_once_each() {
        let store = Arc::new(MemStore::new());
        // channel c1 must exist for delivery fan-out to resolve a Channel
        store
            .create_channel(ChannelInput {
                name: "c1".into(),
                kind: ChannelKind::Webhook,
                url: "http://x".into(),
                secret: None,
                headers: None,
            })
            .await
            .unwrap();
        // rewrite the rule's channel_ids to the created channel id:
        let created = &store.list_channels().await.unwrap()[0];
        let mut r = rule(0);
        r.channel_ids = vec![created.id.clone()];
        let notifier = FakeNotifier::default();
        let mut states: HashMap<String, SeriesState> = HashMap::new();

        // web-01 breaches, web-02 fine → one Triggered notify.
        process_sample(
            &*store,
            &notifier,
            &r,
            &mut states,
            vec![sample("web-01", 0.95), sample("web-02", 0.1)],
            1000,
        )
        .await
        .unwrap();
        assert_eq!(notifier.calls.lock().unwrap().len(), 1);
        assert_eq!(store.list_open_incidents().await.unwrap().len(), 1);

        // web-01 recovers → one Resolved notify; incident closed.
        process_sample(
            &*store,
            &notifier,
            &r,
            &mut states,
            vec![sample("web-01", 0.2), sample("web-02", 0.1)],
            2000,
        )
        .await
        .unwrap();
        let open_incidents = store.list_open_incidents().await.unwrap().len();
        let calls = notifier.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].1, "resolved");
        assert_eq!(open_incidents, 0);
    }

    /// Fix 2/3: a Triggered series that vanishes from a later sample must resolve with its labels
    /// reconstructed into the delivered payload (not `"series": {}`), and must be dropped from the
    /// state map (not retained as `Ok`) so a churning `group_by` can't leak state forever.
    #[tokio::test]
    async fn vanished_triggered_series_resolves_with_labels_and_is_removed() {
        // A notifier that captures the full payload so we can assert the resolve's `series` labels.
        #[derive(Default)]
        struct CapturingNotifier {
            payloads: std::sync::Mutex<Vec<serde_json::Value>>,
        }
        #[async_trait::async_trait]
        impl Notifier for CapturingNotifier {
            async fn deliver(&self, _channel: &Channel, payload: serde_json::Value) {
                self.payloads.lock().unwrap().push(payload);
            }
        }

        let store = Arc::new(MemStore::new());
        store
            .create_channel(ChannelInput {
                name: "c1".into(),
                kind: ChannelKind::Webhook,
                url: "http://x".into(),
                secret: None,
                headers: None,
            })
            .await
            .unwrap();
        let created = &store.list_channels().await.unwrap()[0];
        let mut r = rule(0);
        r.channel_ids = vec![created.id.clone()];
        let notifier = CapturingNotifier::default();
        let mut states: HashMap<String, SeriesState> = HashMap::new();

        // web-01 breaches → Triggered, incident open, state retained.
        process_sample(
            &*store,
            &notifier,
            &r,
            &mut states,
            vec![sample("web-01", 0.95)],
            1000,
        )
        .await
        .unwrap();
        let key = "host.name=web-01".to_string();
        assert!(states.contains_key(&key));
        assert_eq!(store.list_open_incidents().await.unwrap().len(), 1);

        // The series vanishes entirely from the next successful sample → resolve.
        process_sample(&*store, &notifier, &r, &mut states, vec![], 2000)
            .await
            .unwrap();

        // Incident closed…
        assert_eq!(store.list_open_incidents().await.unwrap().len(), 0);

        // …the resolve payload carries the reconstructed series labels (Fix 2)…
        let payloads = notifier.payloads.lock().unwrap();
        let resolve = payloads
            .iter()
            .find(|p| p["status"] == "resolved")
            .expect("a resolved payload was delivered");
        assert_eq!(resolve["series"]["host.name"].as_str(), Some("web-01"));

        // …and the vanished series is dropped from the state map, not retained as Ok (Fix 3).
        assert!(
            !states.contains_key(&key),
            "vanished series must be removed from the state map"
        );
    }
}
