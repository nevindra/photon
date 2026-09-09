//! Docker fleet management end to end, with a FAKE agent: the API server over an in-memory
//! `DeployStore`, an admin session on one side, and on the other a client that speaks the
//! agent protocol (`POST /api/agent/sync` with the per-host token). Covers enrolment, the
//! inventory round trip, a one-shot action with its result and audit rows, roles (403 → PATCH →
//! allowed), a template deploy through render/dry-run/desired-state/secrets/stack-result, and a
//! rollout driven by the supervisor tick.

use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::Arc;
use std::time::Duration;

use photon_api::deploy::{DeployApi, DeployHub, IngestHint, PlainCipher};
use photon_api::users::{SqliteUserStore, UserStore};
use photon_api::ApiServer;
use photon_core::metric_schema::MetricSchema;
use photon_core::schema::LogSchema;
use photon_core::span_schema::SpanSchema;
use photon_deploy::model::*;
use photon_deploy::proto::*;
use photon_deploy::store::MemStore;
use photon_query::{MetricsQueryEngine, QueryEngine, SpanQueryEngine};
use serde_json::{json, Value};

const SESSION_SECRET: &str = "a-long-random-session-signing-secret-value-for-deploy-e2e";

fn free_addr() -> SocketAddr {
    let l = StdTcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

async fn wait_until_connectable(addr: SocketAddr) {
    tokio::time::timeout(Duration::from_secs(5), async move {
        loop {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("server never became connectable on {addr}"));
}

struct Server {
    base: String,
    api: DeployApi,
    _tmp: tempfile::TempDir,
}

impl Server {
    fn url(&self, p: &str) -> String {
        format!("{}{}", self.base, p)
    }
}

async fn spawn_server() -> Server {
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();
    let query =
        QueryEngine::new(hot.clone(), LogSchema::new(&["service.name".to_string()])).unwrap();
    let span_query =
        SpanQueryEngine::new(hot.clone(), SpanSchema::new(&["service.name".to_string()])).unwrap();
    let metrics_query = MetricsQueryEngine::new(
        hot.clone(),
        MetricSchema::new(&["service.name".to_string(), "host.name".to_string()]),
    )
    .unwrap();
    let user_store = SqliteUserStore::open(hot.join("photon.db").to_str().unwrap()).unwrap();
    let api = DeployApi {
        store: Arc::new(MemStore::new()),
        cipher: Arc::new(PlainCipher),
        hub: Arc::new(DeployHub::new()),
        bootstrapper: None,
        agent_binaries_dir: None,
        ingest: Some(IngestHint {
            metrics_endpoint: "http://photon.test:4318/v1/metrics".into(),
            logs_endpoint: "http://photon.test:4318/v1/logs".into(),
            token: "ingest-token".into(),
        }),
        latest_agent_version: Some("9.9.9".into()),
        server_origin: Some("http://photon.test".into()),
        deployments: None,
    };
    let server = ApiServer::new(
        query,
        span_query,
        metrics_query,
        Arc::new(user_store) as Arc<dyn UserStore>,
        SESSION_SECRET,
    )
    .with_deploy(Some(api.clone()));
    let addr = free_addr();
    tokio::spawn(async move {
        server.serve(addr).await.unwrap();
    });
    wait_until_connectable(addr).await;
    Server {
        base: format!("http://{addr}"),
        api,
        _tmp: tmp,
    }
}

async fn setup_admin(c: &reqwest::Client, s: &Server) -> String {
    let r = c
        .post(s.url("/api/setup"))
        .json(&json!({ "username": "admin", "password": "hunter2-long-enough" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let cookie = r
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["role"], "admin", "the first account is the admin");
    cookie
}

async fn login(c: &reqwest::Client, s: &Server, u: &str, p: &str) -> (String, Value) {
    let r = c
        .post(s.url("/api/login"))
        .json(&json!({ "username": u, "password": p }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let cookie = r
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body: Value = r.json().await.unwrap();
    (cookie, body)
}

fn container(
    id: &str,
    name: &str,
    image: &str,
    project: Option<&str>,
    service: Option<&str>,
) -> Container {
    Container {
        id: id.into(),
        name: name.into(),
        image: image.into(),
        image_id: "sha256:abc".into(),
        command: "/run".into(),
        state: ContainerState::Running,
        health: ContainerHealth::None,
        status_text: "Up 2 minutes".into(),
        created_ms: 1,
        started_ms: Some(2),
        finished_ms: None,
        exit_code: None,
        restart_count: 0,
        restart_policy: "always".into(),
        ports: vec![],
        mounts: vec![],
        networks: vec!["bridge".into()],
        labels: Default::default(),
        env_keys: vec![],
        env: Default::default(),
        compose_project: project.map(|s| s.into()),
        compose_service: service.map(|s| s.into()),
        managed_stack_id: None,
        cpu_pct: Some(1.5),
        mem_bytes: Some(1024),
        mem_limit_bytes: None,
        net_rx_bps: None,
        net_tx_bps: None,
        block_read_bps: None,
        block_write_bps: None,
    }
}

fn sync_req(known: u64, containers: Vec<Container>, stacks: Vec<ObservedStack>) -> SyncRequest {
    SyncRequest {
        protocol: PROTOCOL_VERSION,
        agent_version: "0.2.0".into(),
        host: HostIdentity {
            name: "edge-1".into(),
            id: Some("machine-1".into()),
            arch: "x86_64".into(),
            os: "linux".into(),
        },
        capabilities: vec![
            AgentCapability::Read,
            AgentCapability::Lifecycle,
            AgentCapability::Apply,
        ],
        known_generation: known,
        observed: Some(Observed {
            facts: Some(DockerFacts {
                version: "27.1.1".into(),
                api_version: "1.46".into(),
                storage_driver: "overlay2".into(),
                os: "Ubuntu".into(),
                arch: "x86_64".into(),
                containers_running: containers.len() as u32,
                containers_total: containers.len() as u32,
                images: 3,
                disk: DockerDisk {
                    total_bytes: Some(200 << 30),
                    used_bytes: Some(50 << 30),
                    ..Default::default()
                },
            }),
            containers,
            images: vec![],
            volumes: vec![],
            networks: vec![],
            stacks,
            prune_preview: None,
        }),
        results: vec![],
        stack_results: vec![],
        events: vec![DockerEvent {
            id: "e1".into(),
            ts_ms: 5,
            kind: "container".into(),
            action: "start".into(),
            actor: "web".into(),
            attributes: Default::default(),
        }],
        wait_secs: 0,
        active_streams: vec![],
    }
}

async fn agent_sync(
    c: &reqwest::Client,
    s: &Server,
    token: &str,
    req: &SyncRequest,
) -> SyncResponse {
    let r = c
        .post(s.url("/api/agent/sync"))
        .bearer_auth(token)
        .json(req)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
    r.json().await.unwrap()
}

#[tokio::test]
async fn enrol_sync_action_roles_and_audit() {
    let s = spawn_server().await;
    let c = reqwest::Client::new();

    let unauth = c.get(s.url("/api/agents")).send().await.unwrap();
    assert_eq!(
        unauth.status(),
        401,
        "/api/agents must be mounted (401), not absent (404)"
    );
    let admin = setup_admin(&c, &s).await;

    // Enrol: the token is shown once.
    let r = c.post(s.url("/api/agents")).header(reqwest::header::COOKIE, &admin).json(&json!({ "host": "edge-1", "site": "divtik", "capabilities": ["lifecycle", "apply"], "poll_secs": 15 })).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("ak_"));
    assert_eq!(body["host"]["status"], "pending");
    assert_eq!(
        body["host"]["capabilities"],
        json!(["read", "lifecycle", "apply"]),
        "read is implied"
    );

    // A wrong token is a 401; the right one syncs.
    let bad = c
        .post(s.url("/api/agent/sync"))
        .bearer_auth("ak_nope_nope_nope_nope")
        .json(&sync_req(0, vec![], vec![]))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 401);
    let resp = agent_sync(
        &c,
        &s,
        &token,
        &sync_req(
            0,
            vec![container("c1", "web", "nginx:1.27", None, None)],
            vec![],
        ),
    )
    .await;
    assert_eq!(resp.generation, 0);
    assert!(resp.desired.is_none());
    assert!(resp.actions.is_empty());
    assert_eq!(resp.next_poll_secs, 15);
    assert_eq!(resp.ingest.as_ref().unwrap().token, "ingest-token");
    assert_eq!(
        resp.upgrade.as_ref().map(|u| u.version.clone()),
        None,
        "no binary on this server → no upgrade advertised"
    );

    // The host is online and its inventory is served.
    let h: Value = c
        .get(s.url("/api/agents/edge-1"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(h["status"], "online");
    assert_eq!(h["agent_version"], "0.2.0");
    assert_eq!(h["docker"]["version"], "27.1.1");
    let inv: Value = c
        .get(s.url("/api/agents/edge-1/inventory"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(inv["containers"][0]["name"], "web");
    assert_eq!(inv["stale"], false);
    let ev: Value = c
        .get(s.url("/api/agents/edge-1/events"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["events"][0]["action"], "start");
    let st: Value = c
        .get(s.url("/api/agents/edge-1/containers/c1/stats?start=0&end=9999999999999999999"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        st["cpu_pct"].as_array().unwrap().len(),
        1,
        "the sync's sample lands in the stats ring"
    );

    // Roles: a viewer may not act; an operator may.
    let r = c
        .post(s.url("/api/users"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "username": "rina", "password": "rina-password-1", "role": "viewer" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let (rina, body) = login(&c, &s, "rina", "rina-password-1").await;
    assert_eq!(body["role"], "viewer");
    let r = c
        .post(s.url("/api/agents/edge-1/actions"))
        .header(reqwest::header::COOKIE, &rina)
        .json(&json!({ "kind": "restart", "container_id": "c1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"], "Requires the Operator role");
    let r = c
        .patch(s.url("/api/users/rina"))
        .header(reqwest::header::COOKIE, &rina)
        .json(&json!({ "role": "admin" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "a viewer cannot promote themselves");
    let r = c
        .patch(s.url("/api/users/rina"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "role": "operator" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = c
        .patch(s.url("/api/users/admin"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "role": "viewer" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400, "the last admin cannot be demoted");
    let (rina, body) = login(&c, &s, "rina", "rina-password-1").await;
    assert_eq!(body["role"], "operator");

    // A one-shot action: queued, handed to the agent on sync, reported back, audited.
    let r = c
        .post(s.url("/api/agents/edge-1/actions"))
        .header(reqwest::header::COOKIE, &rina)
        .json(&json!({ "kind": "restart", "container_id": "c1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["action"]["status"], "queued");
    assert_eq!(body["action"]["container_name"], "web");
    let action_id = body["action"]["id"].as_str().unwrap().to_string();
    let dup = c
        .post(s.url("/api/agents/edge-1/actions"))
        .header(reqwest::header::COOKIE, &rina)
        .json(&json!({ "kind": "stop", "container_id": "c1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409, "one open action per container");

    let resp = agent_sync(
        &c,
        &s,
        &token,
        &sync_req(
            0,
            vec![container("c1", "web", "nginx:1.27", None, None)],
            vec![],
        ),
    )
    .await;
    assert_eq!(resp.actions.len(), 1);
    assert_eq!(resp.actions[0].id, action_id);
    assert_eq!(resp.actions[0].kind, ActionKind::Restart);
    let mut req = sync_req(
        0,
        vec![container("c1", "web", "nginx:1.27", None, None)],
        vec![],
    );
    req.results = vec![ActionResult {
        action_id: action_id.clone(),
        status: ActionStatus::Done,
        detail: Some("exit 0 in 1.2s".into()),
        started_ms: Some(10),
        finished_ms: Some(20),
    }];
    let resp = agent_sync(&c, &s, &token, &req).await;
    assert!(
        resp.actions.is_empty(),
        "a finished action is not handed out again"
    );
    let acts: Value = c
        .get(s.url("/api/agents/edge-1/actions"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(acts["actions"][0]["status"], "done");
    assert_eq!(acts["actions"][0]["result"], "exit 0 in 1.2s");

    // Volume removal: validated against the last inventory — unknown and in-use volumes are
    // refused now, an unused one is queued with its name on the spec and in the audit row.
    let mut req = sync_req(
        0,
        vec![container("c1", "web", "nginx:1.27", None, None)],
        vec![],
    );
    let vol = |name: &str, used_by: Vec<&str>| VolumeInfo {
        name: name.into(),
        driver: "local".into(),
        mountpoint: format!("/var/lib/docker/volumes/{name}/_data"),
        size_bytes: Some(1 << 30),
        used_by: used_by.into_iter().map(String::from).collect(),
        created_ms: 0,
    };
    req.observed.as_mut().unwrap().volumes =
        vec![vol("pgdata", vec!["web"]), vol("orphan", vec![])];
    agent_sync(&c, &s, &token, &req).await;
    for (body, why) in [
        (json!({ "kind": "volume_remove" }), "volume is required"),
        (
            json!({ "kind": "volume_remove", "volume": "nope" }),
            "no volume nope",
        ),
        (
            json!({ "kind": "volume_remove", "volume": "pgdata" }),
            "pgdata is used by web",
        ),
    ] {
        let r = c
            .post(s.url("/api/agents/edge-1/actions"))
            .header(reqwest::header::COOKIE, &rina)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400, "{why}");
        let e: Value = r.json().await.unwrap();
        assert!(e["error"].as_str().unwrap().contains(why), "{e}");
    }
    let r = c
        .post(s.url("/api/agents/edge-1/actions"))
        .header(reqwest::header::COOKIE, &rina)
        .json(&json!({ "kind": "volume_remove", "volume": "orphan" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["action"]["volume"], "orphan");
    assert!(body["action"]["container_id"].is_null());
    let resp = agent_sync(&c, &s, &token, &req).await;
    assert_eq!(resp.actions.len(), 1);
    assert_eq!(resp.actions[0].kind, ActionKind::VolumeRemove);
    assert_eq!(resp.actions[0].volume.as_deref(), Some("orphan"));
    let vol_audit: Value = c
        .get(s.url("/api/audit?action=volume"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let e = &vol_audit["entries"][0];
    assert_eq!(e["action"], "volume.remove");
    assert_eq!(e["result"], "pending");
    assert_eq!(e["target"]["volume"], "orphan");

    let audit: Value = c
        .get(s.url("/api/audit?action=container"))
        .header(reqwest::header::COOKIE, &rina)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let entries = audit["entries"].as_array().unwrap();
    assert!(entries.iter().any(|e| e["action"] == "container.restart"
        && e["result"] == "ok"
        && e["actor"] == "rina"));
    assert!(entries
        .iter()
        .any(|e| e["action"] == "container.restart" && e["result"] == "pending"));
    let denied: Value = c
        .get(s.url("/api/audit?result=denied"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        denied["total"], 0,
        "a 403 from the role gate never reaches the host, so it is not a denied action row"
    );

    // Rotate: the old token dies at the next sync.
    let r = c
        .post(s.url("/api/agents/edge-1/rotate-key"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap();
    let body: Value = r.json().await.unwrap();
    let token2 = body["token"].as_str().unwrap().to_string();
    let old = c
        .post(s.url("/api/agent/sync"))
        .bearer_auth(&token)
        .json(&sync_req(0, vec![], vec![]))
        .send()
        .await
        .unwrap();
    assert_eq!(old.status(), 401);
    let _ = agent_sync(&c, &s, &token2, &sync_req(0, vec![], vec![])).await;
}

const MANIFEST: &str = r#"{
  "id": "demo", "version": "1.0.0", "name": "Demo app", "description": "d", "icon": "", "categories": ["demo"],
  "variables": [
    {"name": "APP_VERSION", "label": "Version", "type": "string", "required": true, "default": "1.0.0", "preset": true},
    {"name": "API_PORT", "label": "API port", "type": "port", "required": true, "default": "8081"},
    {"name": "DB_PASSWORD", "label": "DB password", "type": "generated", "required": true},
    {"name": "LICENSE_KEY", "label": "License", "type": "secret", "required": true}
  ],
  "requires": {"arch": ["x86_64"], "min_docker": "24.0"},
  "ports": [{"name": "api", "container": 8080, "default_host": 8081, "protocol": "tcp"}],
  "volumes": [], "health": null,
  "observability": {"otlp_endpoint": true, "ingest_token": true, "service_name": null, "environment": false}
}"#;

const COMPOSE: &str = "services:\n  api:\n    image: ghcr.io/nodeflux/api:${APP_VERSION}\n    ports:\n      - \"${API_PORT}:8080\"\n    environment:\n      DB_PASSWORD: ${DB_PASSWORD}\n      LICENSE_KEY: ${LICENSE_KEY}\n  db:\n    image: postgres:16\n";

#[tokio::test]
async fn template_deploy_and_rollout_reach_the_agent() {
    let s = spawn_server().await;
    let c = reqwest::Client::new();
    let admin = setup_admin(&c, &s).await;
    let r = c
        .post(s.url("/api/agents"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "host": "edge-1", "capabilities": ["apply"] }))
        .send()
        .await
        .unwrap();
    let token = r.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = agent_sync(&c, &s, &token, &sync_req(0, vec![], vec![])).await;

    // Publish; a second publish of the same version is refused (immutability).
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let r = c
        .post(s.url("/api/templates/versions"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "template_id": null, "manifest": manifest, "compose_text": COMPOSE }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let body: Value = r.json().await.unwrap();
    let vid = body["version"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["template"]["latest_version"], "1.0.0");
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let again = c
        .post(s.url("/api/templates/versions"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "template_id": "demo", "manifest": manifest, "compose_text": COMPOSE }))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 409);

    // Dry run: renders, checks, names the secrets.
    let deploy_input = json!({ "template_version_id": vid, "host": "edge-1", "name": "demo", "variables": { "API_PORT": "9090" }, "secrets": { "LICENSE_KEY": "NFX-1" }, "deploy_options": { "pull": "missing", "force_recreate": false, "recreate_volumes": false }, "acknowledged_warnings": [] });
    let dry: Value = c
        .post(s.url("/api/templates/dry-run"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&deploy_input)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(dry["ok"], true, "{dry}");
    assert!(dry["compose"]
        .as_str()
        .unwrap()
        .contains("ghcr.io/nodeflux/api:1.0.0"));
    assert!(dry["compose"].as_str().unwrap().contains("\"9090:8080\""));
    assert!(dry["env"]
        .as_str()
        .unwrap()
        .contains("OTEL_EXPORTER_OTLP_ENDPOINT=http://photon.test:4318"));
    assert_eq!(dry["secrets"], json!(["LICENSE_KEY"]));
    assert_eq!(dry["generated"], json!(["DB_PASSWORD"]));
    assert!(dry["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|ch| ch["key"] == "docker" && ch["status"] == "pass"));

    // Deploy: the stack is applying; the agent gets desired state on its next sync.
    let r = c
        .post(s.url("/api/templates/deploy"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&deploy_input)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let body: Value = r.json().await.unwrap();
    let stack_id = body["stack"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["stack"]["status"], "applying");
    assert_eq!(body["stack"]["desired_generation"], 1);
    assert_eq!(
        body["stack"]["secrets"],
        json!(["DB_PASSWORD", "LICENSE_KEY"])
    );
    assert_eq!(body["stack"]["services"][0]["name"], "api");

    let resp = agent_sync(&c, &s, &token, &sync_req(0, vec![], vec![])).await;
    let desired = resp.desired.expect("desired state for a stale generation");
    assert_eq!(desired.stacks.len(), 1);
    let ds = &desired.stacks[0];
    assert_eq!(ds.id, stack_id);
    assert_eq!(ds.op, StackOp::Up);
    assert!(
        ds.files.iter().any(|f| f.path == ".env"
            && f.content.contains("LICENSE_KEY=")
            && !f.content.contains("NFX-1")),
        "secret values never ride in desired state"
    );
    assert_eq!(ds.secret_names, vec!["DB_PASSWORD", "LICENSE_KEY"]);
    let sec: SecretsResponse = c
        .post(s.url("/api/agent/secrets"))
        .bearer_auth(&token)
        .json(&SecretsRequest {
            stack_id: stack_id.clone(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sec.values["LICENSE_KEY"], "NFX-1");
    assert_eq!(
        sec.values["DB_PASSWORD"].len(),
        32,
        "generated once, server-side"
    );
    let none = c
        .post(s.url("/api/agent/secrets"))
        .json(&SecretsRequest {
            stack_id: stack_id.clone(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(none.status(), 401);

    // The agent applies and reports; the stack is running with the drift baseline recorded.
    let running = vec![
        container(
            "a1",
            "demo-api-1",
            "ghcr.io/nodeflux/api:1.0.0",
            Some("demo"),
            Some("api"),
        ),
        container("d1", "demo-db-1", "postgres:16", Some("demo"), Some("db")),
    ];
    let mut req = sync_req(
        resp.generation,
        running.clone(),
        vec![ObservedStack {
            name: "demo".into(),
            applied_generation: Some(1),
            config_hashes: [
                ("api".to_string(), "h1".to_string()),
                ("db".to_string(), "h2".to_string()),
            ]
            .into(),
            working_dir: None,
            compose_file: None,
        }],
    );
    req.stack_results = vec![StackResult {
        stack_id: stack_id.clone(),
        generation: 1,
        op: StackOp::Up,
        ok: true,
        detail: Some("done in 3.0s".into()),
        finished_ms: 100,
        config_hashes: [
            ("api".to_string(), "h1".to_string()),
            ("db".to_string(), "h2".to_string()),
        ]
        .into(),
    }];
    let resp = agent_sync(&c, &s, &token, &req).await;
    assert!(
        resp.desired.is_none(),
        "current generation → nothing to send"
    );
    let st: Value = c
        .get(s.url(&format!("/api/stacks/{stack_id}")))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["status"], "running", "{st}");
    assert_eq!(st["applied_generation"], 1);
    assert_eq!(st["services"][0]["containers"][0]["name"], "demo-api-1");
    assert_eq!(st["apm_services"], json!(["api", "db"]));
    assert!(st["drift"].as_array().unwrap().is_empty());

    // Drift: the image on the host changed and a container vanished.
    let drifted = vec![container(
        "a1",
        "demo-api-1",
        "ghcr.io/nodeflux/api:0.9.0",
        Some("demo"),
        Some("api"),
    )];
    let _ = agent_sync(
        &c,
        &s,
        &token,
        &sync_req(
            resp.generation,
            drifted,
            vec![ObservedStack {
                name: "demo".into(),
                applied_generation: Some(1),
                config_hashes: [("api".to_string(), "h1".to_string())].into(),
                working_dir: None,
                compose_file: None,
            }],
        ),
    )
    .await;
    let st: Value = c
        .get(s.url(&format!("/api/stacks/{stack_id}")))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["status"], "drifted");
    let kinds: Vec<&str> = st["drift"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap())
        .collect();
    assert!(
        kinds.contains(&"image_changed") && kinds.contains(&"container_missing"),
        "{kinds:?}"
    );
    // Reconcile is a deliberate act: it bumps the generation and the agent re-applies.
    let r = c
        .post(s.url(&format!("/api/stacks/{stack_id}/reconcile")))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let resp = agent_sync(&c, &s, &token, &sync_req(resp.generation, vec![], vec![])).await;
    assert_eq!(resp.desired.unwrap().stacks[0].op, StackOp::Reconcile);

    // Rollout of the same version to the host: the supervisor applies the canary, the agent
    // reports, the target turns healthy, the rollout completes (one host → one stage).
    let r = c.post(s.url("/api/rollouts")).header(reqwest::header::COOKIE, &admin).json(&json!({
        "name": "demo everywhere", "template_version_id": vid, "stack_name": "demo2",
        "selector": { "kind": "hosts", "hosts": ["edge-1"] },
        "strategy": { "canary": 1, "canary_is_percent": false, "failure_threshold": 1, "prepull": false, "auto_promote": false, "health_grace_secs": 0 },
        "variables": { "API_PORT": "9191" }, "secrets": { "LICENSE_KEY": "NFX-2" },
        "deploy_options": { "pull": "missing", "force_recreate": false, "recreate_volumes": false }, "acknowledged_warnings": [], "start": true
    })).send().await.unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let body: Value = r.json().await.unwrap();
    let rid = body["rollout"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["rollout"]["state"], "running");
    assert_eq!(body["rollout"]["targets"][0]["status"], "queued");

    photon_api::deploy::supervisor::tick(&s.api).await.unwrap();
    let ro: Value = c
        .get(s.url(&format!("/api/rollouts/{rid}")))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ro["targets"][0]["status"], "applying", "{ro}");
    let sid2 = ro["targets"][0]["stack_id"].as_str().unwrap().to_string();
    let resp = agent_sync(&c, &s, &token, &sync_req(0, vec![], vec![])).await;
    let d = resp.desired.unwrap();
    assert!(d.stacks.iter().any(|x| x.id == sid2 && x.name == "demo2"));
    let running2 = vec![
        container(
            "b1",
            "demo2-api-1",
            "ghcr.io/nodeflux/api:1.0.0",
            Some("demo2"),
            Some("api"),
        ),
        container("e1", "demo2-db-1", "postgres:16", Some("demo2"), Some("db")),
    ];
    let mut req = sync_req(
        resp.generation,
        running2,
        vec![ObservedStack {
            name: "demo2".into(),
            applied_generation: Some(1),
            config_hashes: Default::default(),
            working_dir: None,
            compose_file: None,
        }],
    );
    req.stack_results = vec![StackResult {
        stack_id: sid2.clone(),
        generation: 1,
        op: StackOp::Up,
        ok: true,
        detail: None,
        finished_ms: 200,
        config_hashes: Default::default(),
    }];
    let _ = agent_sync(&c, &s, &token, &req).await;
    photon_api::deploy::supervisor::tick(&s.api).await.unwrap();
    let ro: Value = c
        .get(s.url(&format!("/api/rollouts/{rid}")))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ro["targets"][0]["status"], "healthy", "{ro}");
    assert_eq!(ro["state"], "completed");
    let audit: Value = c
        .get(s.url("/api/audit?action=rollout"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["action"] == "rollout.complete"));
}

/// Image registry credentials: an admin stores one (write-only), preflight names it for the
/// registry it covers and warns about the one it does not, the agent gets it decrypted over
/// its own token (and only what its host is scoped for), and a delete takes it away.
#[tokio::test]
async fn registry_credentials_reach_preflight_and_the_agent() {
    let s = spawn_server().await;
    let c = reqwest::Client::new();
    let admin = setup_admin(&c, &s).await;
    let mut tokens = Vec::new();
    for (host, site) in [("edge-1", "jakarta"), ("core-1", "hq")] {
        let r = c
            .post(s.url("/api/agents"))
            .header(reqwest::header::COOKIE, &admin)
            .json(&json!({ "host": host, "site": site, "capabilities": ["apply", "lifecycle"] }))
            .send()
            .await
            .unwrap();
        let token = r.json::<Value>().await.unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string();
        let _ = agent_sync(&c, &s, &token, &sync_req(0, vec![], vec![])).await;
        tokens.push(token);
    }
    let (edge, core) = (&tokens[0], &tokens[1]);

    // Before any credential: the dry run warns about ghcr.io and treats Hub as anonymous.
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let r = c
        .post(s.url("/api/templates/versions"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "template_id": null, "manifest": manifest, "compose_text": COMPOSE }))
        .send()
        .await
        .unwrap();
    let vid = r.json::<Value>().await.unwrap()["version"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let deploy_input = json!({ "template_version_id": vid, "host": "edge-1", "name": "demo", "variables": { "API_PORT": "9090" }, "secrets": { "LICENSE_KEY": "NFX-1" }, "deploy_options": { "pull": "missing", "force_recreate": false, "recreate_volumes": false }, "acknowledged_warnings": [] });
    let check = |dry: &Value, key: &str| -> (String, String) {
        let c = dry["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["key"] == key)
            .unwrap_or_else(|| panic!("no check {key} in {dry}"));
        (
            c["status"].as_str().unwrap().to_string(),
            c["detail"].as_str().unwrap().to_string(),
        )
    };
    let dry: Value = c
        .post(s.url("/api/templates/dry-run"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&deploy_input)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(check(&dry, "registry:ghcr.io").0, "warn");
    assert_eq!(check(&dry, "registry:docker.io").0, "skip");

    // Store one for ghcr.io scoped to the jakarta site. The token never comes back.
    let r = c
        .post(s.url("/api/image-registries"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "server": "https://ghcr.io/", "username": "deploy-bot", "secret": "ghp_secret", "label": "GHCR deploy token", "scope": { "kind": "site", "sites": ["jakarta"] } }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let body: Value = r.json().await.unwrap();
    let cid = body["credential"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["credential"]["server"], "ghcr.io", "normalized");
    assert!(body["credential"].get("secret").is_none());
    let list: Value = c
        .get(s.url("/api/image-registries"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["credentials"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains("ghp_secret"));
    let missing = c
        .post(s.url("/api/image-registries"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "server": "ghcr.io", "username": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 400, "a token is required on create");

    // Preflight now names it.
    let dry: Value = c
        .post(s.url("/api/templates/dry-run"))
        .header(reqwest::header::COOKIE, &admin)
        .json(&deploy_input)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        check(&dry, "registry:ghcr.io"),
        (
            "pass".to_string(),
            "as deploy-bot (GHCR deploy token)".to_string()
        )
    );

    // The agent in the site gets it decrypted; the other site's agent does not; no token, 401.
    async fn auth(
        c: &reqwest::Client,
        s: &Server,
        token: &str,
        servers: Vec<&str>,
    ) -> Vec<RegistryAuth> {
        c.post(s.url("/api/agent/registry-auth"))
            .bearer_auth(token)
            .json(&RegistryAuthRequest {
                servers: servers.into_iter().map(String::from).collect(),
            })
            .send()
            .await
            .unwrap()
            .json::<RegistryAuthResponse>()
            .await
            .unwrap()
            .credentials
    }
    let got = auth(&c, &s, edge, vec![]).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].server, "ghcr.io");
    assert_eq!(got[0].username, "deploy-bot");
    assert_eq!(got[0].secret, "ghp_secret");
    assert!(
        auth(&c, &s, edge, vec!["docker.io"]).await.is_empty(),
        "narrowed away"
    );
    assert!(
        auth(&c, &s, core, vec![]).await.is_empty(),
        "not in the site"
    );
    let anon = c
        .post(s.url("/api/agent/registry-auth"))
        .json(&RegistryAuthRequest::default())
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // Rotate the token and widen the scope; then remove it.
    let r = c
        .patch(s.url(&format!("/api/image-registries/{cid}")))
        .header(reqwest::header::COOKIE, &admin)
        .json(&json!({ "secret": "ghp_rotated", "scope": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let got = auth(&c, &s, core, vec![]).await;
    assert_eq!(got.len(), 1, "every host now");
    assert_eq!(got[0].secret, "ghp_rotated");
    let r = c
        .delete(s.url(&format!("/api/image-registries/{cid}")))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(auth(&c, &s, edge, vec![]).await.is_empty());

    // Every step is in the audit log, with the registry and user but never the token.
    let audit: Value = c
        .get(s.url("/api/audit?action=registry_credential&limit=10"))
        .header(reqwest::header::COOKIE, &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let actions: Vec<&str> = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        vec![
            "registry_credential.delete",
            "registry_credential.update",
            "registry_credential.create"
        ]
    );
    assert!(!audit.to_string().contains("ghp_"));
}
