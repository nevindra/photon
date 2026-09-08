//! Docker daemon events (R2.4): a background subscription to `/events` filtered to
//! container / image / volume / network, buffered (cap `EVENT_CAP`, oldest dropped) and
//! drained into every sync request. An OOM kill, a `die`, a `pull` — each rides the next poll
//! as a `DockerEvent` and the server turns it into a log line on the Attention Board's timeline.
//! The subscription reconnects itself after a daemon restart, resuming from the last event
//! time so nothing in between is lost.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use bollard::models::EventMessage;
use bollard::query_parameters::EventsOptionsBuilder;
use bollard::Docker;
use futures::StreamExt;
use photon_deploy::model::DockerEvent;

pub const EVENT_CAP: usize = 500;

pub struct EventBuffer {
    buf: Arc<Mutex<VecDeque<DockerEvent>>>,
    task: tokio::task::JoinHandle<()>,
}

impl EventBuffer {
    pub fn spawn(docker: Docker) -> EventBuffer {
        let buf: Arc<Mutex<VecDeque<DockerEvent>>> = Arc::new(Mutex::new(VecDeque::new()));
        let sink = buf.clone();
        let task = tokio::spawn(async move {
            let mut since: Option<i64> = None;
            let mut seq: u64 = 0;
            loop {
                let filters: HashMap<&str, Vec<&str>> =
                    HashMap::from([("type", vec!["container", "image", "volume", "network"])]);
                let mut b = EventsOptionsBuilder::new().filters(&filters);
                let since_s;
                if let Some(s) = since {
                    since_s = s.to_string();
                    b = b.since(&since_s);
                }
                let mut stream = docker.events(Some(b.build()));
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(m) => {
                            if let Some(t) = m.time {
                                since = Some(t);
                            }
                            seq += 1;
                            if let Some(ev) = map_event(m, seq) {
                                let mut q = sink.lock().unwrap_or_else(|p| p.into_inner());
                                if q.len() >= EVENT_CAP {
                                    q.pop_front();
                                }
                                q.push_back(ev);
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "photon-agent: docker events stream error: {e}; resubscribing"
                            );
                            break;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
        EventBuffer { buf, task }
    }

    pub fn drain(&self) -> Vec<DockerEvent> {
        let mut q = self.buf.lock().unwrap_or_else(|p| p.into_inner());
        q.drain(..).collect()
    }
}

impl Drop for EventBuffer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One daemon event → the wire shape. `actor` is the human name when Docker attached one
/// (`name` attribute for containers, the image ref for images) and the id otherwise.
pub fn map_event(m: EventMessage, seq: u64) -> Option<DockerEvent> {
    let kind = m.typ.map(|t| t.to_string()).filter(|s| !s.is_empty())?;
    let action = m.action.unwrap_or_default();
    let actor = m.actor.unwrap_or_default();
    let attributes: std::collections::BTreeMap<String, String> =
        actor.attributes.unwrap_or_default().into_iter().collect();
    let id = actor.id.unwrap_or_default();
    let actor_name = attributes
        .get("name")
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if kind == "image" {
                id.clone()
            } else {
                id.chars().take(12).collect()
            }
        });
    let ts_ms = match (m.time_nano, m.time) {
        (Some(n), _) => n / 1_000_000,
        (None, Some(s)) => s * 1000,
        _ => crate::clock::now_ms(),
    };
    Some(DockerEvent {
        id: format!("{ts_ms}-{seq}"),
        ts_ms,
        kind,
        action,
        actor: actor_name,
        attributes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{EventActor, EventMessageTypeEnum};

    #[test]
    fn maps_container_and_image_events() {
        let m = EventMessage {
            typ: Some(EventMessageTypeEnum::CONTAINER),
            action: Some("die".into()),
            actor: Some(EventActor {
                id: Some("0123456789abcdef".into()),
                attributes: Some(HashMap::from([
                    ("name".to_string(), "web".to_string()),
                    ("exitCode".to_string(), "137".to_string()),
                ])),
            }),
            time: Some(1_700_000_000),
            time_nano: Some(1_700_000_000_123_456_789),
            ..Default::default()
        };
        let e = map_event(m, 1).unwrap();
        assert_eq!(e.kind, "container");
        assert_eq!(e.action, "die");
        assert_eq!(e.actor, "web");
        assert_eq!(e.ts_ms, 1_700_000_000_123);
        assert_eq!(e.attributes["exitCode"], "137");
        assert_eq!(e.id, "1700000000123-1");

        let m = EventMessage {
            typ: Some(EventMessageTypeEnum::IMAGE),
            action: Some("pull".into()),
            actor: Some(EventActor {
                id: Some("nginx:latest".into()),
                attributes: None,
            }),
            time: Some(5),
            ..Default::default()
        };
        let e = map_event(m, 2).unwrap();
        assert_eq!(e.actor, "nginx:latest");
        assert_eq!(e.ts_ms, 5000);
        assert!(map_event(EventMessage::default(), 3).is_none());
    }
}
