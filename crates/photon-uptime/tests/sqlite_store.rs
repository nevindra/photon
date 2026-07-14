use photon_uptime::model::*;
use photon_uptime::store::sqlite::SqliteStore;
use photon_uptime::store::UptimeStore;

fn input() -> MonitorInput {
    serde_json::from_str(
        r#"{"name":"api","type":"tcp","target":"127.0.0.1:80",
        "interval_secs":30,"timeout_secs":5,"retries":2}"#,
    )
    .unwrap()
}

#[tokio::test]
async fn sqlite_crud_heartbeats_incidents_retention() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uptime.db");
    let store = SqliteStore::open(path.to_str().unwrap()).unwrap();

    let m = store.create_monitor(input()).await.unwrap();
    assert_eq!(store.list_monitors().await.unwrap().len(), 1);
    assert_eq!(store.get_monitor(&m.id).await.unwrap().unwrap().name, "api");

    store
        .set_monitor_state(&m.id, MonitorState::Up, 5000, 12)
        .await
        .unwrap();
    assert_eq!(
        store.get_monitor(&m.id).await.unwrap().unwrap().last_state,
        MonitorState::Up
    );

    store
        .append_heartbeat(Heartbeat {
            monitor_id: m.id.clone(),
            ts: 1000,
            ok: true,
            latency_ms: 5,
            status_code: None,
            error: None,
        })
        .await
        .unwrap();
    store
        .append_heartbeat(Heartbeat {
            monitor_id: m.id.clone(),
            ts: 2000,
            ok: false,
            latency_ms: 0,
            status_code: None,
            error: Some("x".into()),
        })
        .await
        .unwrap();
    assert_eq!(store.heartbeats(&m.id, 0).await.unwrap().len(), 2);
    assert!((store.uptime_pct(&m.id, 0).await.unwrap() - 50.0).abs() < 1e-6);

    let iid = store.open_incident(&m.id, 2000, "x".into()).await.unwrap();
    assert_eq!(store.open_incident_id(&m.id).await.unwrap(), Some(iid));
    store.close_incident(iid, 3000).await.unwrap();
    assert_eq!(store.open_incident_id(&m.id).await.unwrap(), None);

    assert_eq!(store.prune_heartbeats(1500).await.unwrap(), 1);
    assert!(store.delete_monitor(&m.id).await.unwrap());
    assert!(store.list_monitors().await.unwrap().is_empty());

    // Reopen the same file: schema + data persistence works across connections.
    drop(store);
    let store2 = SqliteStore::open(path.to_str().unwrap()).unwrap();
    assert!(store2.list_monitors().await.unwrap().is_empty());
}

#[tokio::test]
async fn update_monitor_preserves_enabled_when_paused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uptime.db");
    let store = SqliteStore::open(path.to_str().unwrap()).unwrap();

    let m = store.create_monitor(input()).await.unwrap();
    assert!(
        !store
            .set_enabled(&m.id, false)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );

    // The edit payload omits `enabled`, so it deserializes to the `default_true` default —
    // mirroring the frontend's PATCH, which never sends `enabled`. update_monitor must not
    // let that default silently re-enable a paused monitor.
    let mut edit = input();
    edit.name = "renamed".into();
    let up = store.update_monitor(&m.id, edit).await.unwrap().unwrap();
    assert_eq!(up.name, "renamed");
    assert!(!up.enabled, "editing a paused monitor must keep it paused");
}

#[tokio::test]
async fn prune_incidents_and_stats() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uptime.db");
    let store = SqliteStore::open(path.to_str().unwrap()).unwrap();

    let m = store.create_monitor(input()).await.unwrap();

    for ts in [100, 200, 300] {
        store
            .append_heartbeat(Heartbeat {
                monitor_id: m.id.clone(),
                ts,
                ok: true,
                latency_ms: 5,
                status_code: Some(200),
                error: None,
            })
            .await
            .unwrap();
    }

    // Closed incident (started 100, ended 150) -> prunable before 200.
    let closed_id = store
        .open_incident(&m.id, 100, "down".into())
        .await
        .unwrap();
    store.close_incident(closed_id, 150).await.unwrap();

    // Open incident (started 500, never ended) -> never pruned.
    store
        .open_incident(&m.id, 500, "down again".into())
        .await
        .unwrap();

    let stats = store.stats().await.unwrap();
    assert_eq!(stats.monitor_count, 1);
    assert_eq!(stats.heartbeat_count, 3);
    assert_eq!(stats.incident_count, 2);
    assert_eq!(stats.oldest_heartbeat_ts, Some(100));
    assert_eq!(stats.newest_heartbeat_ts, Some(300));

    let removed = store.prune_incidents(200).await.unwrap();
    assert_eq!(removed, 1); // only the closed-before-200 incident

    assert_eq!(store.stats().await.unwrap().incident_count, 1);
}

#[tokio::test]
async fn open_creates_missing_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/sub/uptime.db");
    assert!(!path.parent().unwrap().exists());

    let store = SqliteStore::open(path.to_str().unwrap()).unwrap();
    assert!(store.list_monitors().await.unwrap().is_empty());
    assert!(path.exists());
}
