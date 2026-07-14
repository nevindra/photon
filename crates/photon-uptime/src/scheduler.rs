//! The central scheduler: one task owns per-monitor runtime state + next-due, dispatches
//! probes onto a bounded pool, folds results through the state machine, and applies
//! live-reload commands from the API. A panic in one probe cannot kill the loop.

use crate::model::*;
use crate::notify::{Notifier, NotifyEvent};
use crate::probe::Prober;
use crate::state::apply;
use crate::store::UptimeStore;
use photon_core::PhotonError;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

/// Fold one probe result into durable state: write the heartbeat, update denormalized
/// monitor state, open/close an incident on a transition, and fire the notifier.
pub async fn process_result<S: UptimeStore, N: Notifier>(
    store: &S,
    notifier: &N,
    monitor: &Monitor,
    prev: RuntimeState,
    result: CheckResult,
) -> Result<RuntimeState, PhotonError> {
    let at = now_ms();
    let (next, transition) = apply(prev, &result, monitor.retries);

    store
        .append_heartbeat(Heartbeat {
            monitor_id: monitor.id.clone(),
            ts: at,
            ok: result.ok,
            latency_ms: result.latency_ms,
            status_code: result.status_code,
            error: result.error.clone(),
        })
        .await?;
    store
        .set_monitor_state(&monitor.id, next.state, at, result.latency_ms)
        .await?;

    match transition {
        Some(Transition::WentDown) => {
            store
                .open_incident(
                    &monitor.id,
                    at,
                    result.error.clone().unwrap_or_else(|| "down".into()),
                )
                .await?;
        }
        Some(Transition::Recovered) => {
            if let Some(iid) = store.open_incident_id(&monitor.id).await? {
                store.close_incident(iid, at).await?;
            }
        }
        None => {}
    }
    if let Some(t) = transition {
        let ev = NotifyEvent {
            monitor,
            transition: t,
            at,
            result: &result,
        };
        let _ = notifier.notify(&ev).await; // notifier is already non-fatal, but never propagate
    }
    Ok(next)
}

struct Slot {
    monitor: Monitor,
    state: RuntimeState,
    next_due: i64,
}

/// Run the scheduler forever. Returns only if `cmd_rx` closes (server shutdown).
pub async fn run<S: UptimeStore, P: Prober + 'static, N: Notifier + 'static>(
    store: Arc<S>,
    prober: Arc<P>,
    notifier: Arc<N>,
    mut cmd_rx: mpsc::Receiver<SchedulerCommand>,
    concurrency: usize,
) {
    let mut slots: HashMap<MonitorId, Slot> = HashMap::new();
    // Seed from the store; runtime state starts from the persisted last_state.
    match store.list_monitors().await {
        Ok(ms) => {
            for m in ms {
                let state = RuntimeState::from_state(m.last_state);
                slots.insert(
                    m.id.clone(),
                    Slot {
                        monitor: m,
                        state,
                        next_due: now_ms(),
                    },
                );
            }
        }
        Err(e) => eprintln!("uptime scheduler: initial load failed: {e}"),
    }

    let sem = Arc::new(Semaphore::new(concurrency.max(1)));
    // Probe results flow back here so all state mutation stays single-threaded.
    let (done_tx, mut done_rx) = mpsc::channel::<(MonitorId, CheckResult)>(1024);
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = now_ms();
                for slot in slots.values_mut() {
                    if !slot.monitor.enabled || slot.next_due > now { continue; }
                    slot.next_due = now + slot.monitor.interval_secs.max(1) as i64 * 1000;
                    let (prober, sem, done_tx, monitor) = (prober.clone(), sem.clone(), done_tx.clone(), slot.monitor.clone());
                    tokio::spawn(async move {
                        let Ok(_permit) = sem.acquire_owned().await else { return };
                        let result = prober.probe(&monitor).await;
                        let _ = done_tx.send((monitor.id, result)).await;
                    });
                }
            }
            Some((id, result)) = done_rx.recv() => {
                if let Some(slot) = slots.get_mut(&id) {
                    match process_result(&*store, &*notifier, &slot.monitor, slot.state, result).await {
                        Ok(next) => slot.state = next,
                        Err(e) => eprintln!("uptime scheduler: process {id}: {e}"),
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SchedulerCommand::Upsert(m)) => {
                        let state = slots.get(&m.id).map(|s| s.state).unwrap_or_else(|| RuntimeState::from_state(m.last_state));
                        slots.insert(m.id.clone(), Slot { monitor: *m, state, next_due: now_ms() });
                    }
                    Some(SchedulerCommand::Remove(id)) => { slots.remove(&id); }
                    None => break, // channel closed → shutdown
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::FakeNotifier;
    use crate::store::MemStore;
    use std::sync::Arc;

    fn res(ok: bool) -> CheckResult {
        CheckResult {
            ok,
            latency_ms: 7,
            status_code: Some(if ok { 200 } else { 500 }),
            error: if ok { None } else { Some("bad".into()) },
        }
    }

    #[tokio::test]
    async fn going_down_opens_incident_writes_hb_and_notifies_once() {
        let store = Arc::new(MemStore::new());
        let notifier = Arc::new(FakeNotifier::default());
        let input: MonitorInput = serde_json::from_str(r#"{"name":"api","type":"http","target":"https://x.test","interval_secs":30,"timeout_secs":5,"retries":1}"#).unwrap();
        let m = store.create_monitor(input).await.unwrap();

        // retries=1 → first failure goes DOWN.
        let mut st = RuntimeState::from_state(MonitorState::Up);
        st = process_result(&*store, &*notifier, &m, st, res(false))
            .await
            .unwrap();
        assert_eq!(st.state, MonitorState::Down);
        assert_eq!(store.heartbeats(&m.id, 0).await.unwrap().len(), 1);
        assert!(store.open_incident_id(&m.id).await.unwrap().is_some());
        assert_eq!(notifier.calls.lock().unwrap().len(), 1);
        assert_eq!(
            store.get_monitor(&m.id).await.unwrap().unwrap().last_state,
            MonitorState::Down
        );

        // recovery closes the incident + notifies recovery.
        st = process_result(&*store, &*notifier, &m, st, res(true))
            .await
            .unwrap();
        assert_eq!(st.state, MonitorState::Up);
        assert_eq!(store.open_incident_id(&m.id).await.unwrap(), None);
        let calls = notifier.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].1, Transition::Recovered);
    }
}
