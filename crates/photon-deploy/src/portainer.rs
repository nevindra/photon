//! Portainer app-template JSON (v2 / v3) → Photon templates. A port of
//! `frontend/src/lib/deploy/portainer.ts`. Gives us the community catalog for free (R5.7):
//! compose stacks (type 3) become a template whose compose file is fetched on sync; the `env`
//! array becomes typed variables (Portainer's `select` and `preset` survive; a name that looks
//! like a password/token is a secret, a port is a port, a path is a path, everything else is
//! text). Container templates (type 1) are converted into a one-service compose file; Swarm
//! stacks (type 2) are skipped, Photon does not cluster. Pure, no I/O.

use crate::model::{
    TemplateManifest, TemplateObservability, TemplateRequires, TemplateVariable, VariableType,
};
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PortainerSelect {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub default: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PortainerEnv {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub preset: bool,
    #[serde(default)]
    pub select: Option<Vec<PortainerSelect>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PortainerRepository {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub stackfile: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PortainerVolume {
    #[serde(default)]
    pub container: String,
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub readonly: bool,
}

/// One entry of the `templates` array, with everything optional but `type` — Portainer's own
/// catalog is not uniform.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PortainerTemplate {
    #[serde(rename = "type", default)]
    pub kind: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub categories: Option<Vec<String>>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub repository: Option<PortainerRepository>,
    #[serde(default)]
    pub env: Option<Vec<PortainerEnv>>,
    #[serde(default)]
    pub ports: Option<Vec<String>>,
    #[serde(default)]
    pub volumes: Option<Vec<PortainerVolume>>,
    #[serde(default)]
    pub restart_policy: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSourceKind {
    /// Type 3: the compose file lives in the template's repository and is fetched on sync.
    ComposeStack,
    /// Type 1: a single container, synthesized into a one-service compose file.
    Container,
}

#[derive(Debug, Clone)]
pub struct ImportedTemplate {
    pub manifest: TemplateManifest,
    pub compose_text: String,
    pub source_kind: ImportSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct ImportReport {
    pub imported: Vec<ImportedTemplate>,
    pub skipped: Vec<Skipped>,
    /// 2 or 3 — the catalog's declared format.
    pub version: u8,
}

/// Lowercase, runs of anything but `[a-z0-9]` collapsed to `-`, trimmed, at most 48 chars;
/// `template` when nothing survives.
pub fn slug(s: &str) -> String {
    let lowered = s.to_lowercase();
    let mut out = String::new();
    let mut pending_dash = false;
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    let trimmed: String = out.trim_matches('-').chars().take(48).collect();
    if trimmed.is_empty() {
        "template".to_string()
    } else {
        trimmed
    }
}

fn contains_ci(hay: &str, needles: &[&str]) -> bool {
    let h = hay.to_ascii_lowercase();
    needles.iter().any(|n| h.contains(n))
}

fn to_variable(e: &PortainerEnv) -> TemplateVariable {
    let sel = e.select.as_deref().filter(|s| !s.is_empty());
    let kind = if sel.is_some() {
        VariableType::Select
    } else if contains_ci(&e.name, &["pass", "secret", "token", "key"]) {
        VariableType::Secret
    } else if contains_ci(&e.name, &["port"]) {
        VariableType::Port
    } else if contains_ci(&e.name, &["path", "dir"]) {
        VariableType::Path
    } else {
        VariableType::String
    };
    let has_default = e.default.as_deref().is_some_and(|d| !d.is_empty());
    let default = match sel {
        Some(options) => Some(
            options
                .iter()
                .find(|o| o.default)
                .unwrap_or(&options[0])
                .value
                .clone(),
        ),
        None => e.default.clone(),
    };
    TemplateVariable {
        name: e.name.clone(),
        label: e.label.clone().unwrap_or_else(|| e.name.clone()),
        description: e.description.clone(),
        kind,
        required: !has_default && sel.is_none(),
        default,
        preset: Some(e.preset),
        options: sel.map(|s| s.iter().map(|o| o.value.clone()).collect()),
        pattern: None,
        group: None,
    }
}

/// The one-service compose file for a container template (type 1).
fn container_compose(t: &PortainerTemplate) -> String {
    let svc = slug(t.name.as_deref().or(t.title.as_deref()).unwrap_or(""));
    let mut lines: Vec<String> = vec![
        "services:".to_string(),
        format!("  {svc}:"),
        format!("    image: {}", t.image.as_deref().unwrap_or("unknown")),
        format!(
            "    restart: {}",
            t.restart_policy.as_deref().unwrap_or("unless-stopped")
        ),
    ];
    if let Some(ports) = t.ports.as_ref().filter(|p| !p.is_empty()) {
        lines.push("    ports:".to_string());
        for p in ports {
            lines.push(format!("      - \"{p}\""));
        }
    }
    let volumes = t.volumes.as_deref().unwrap_or(&[]);
    if !volumes.is_empty() {
        lines.push("    volumes:".to_string());
        for v in volumes {
            let src = v
                .bind
                .clone()
                .unwrap_or_else(|| format!("{}_data", slug(&v.container)));
            let ro = if v.readonly { ":ro" } else { "" };
            lines.push(format!("      - {src}:{}{ro}", v.container));
        }
    }
    if let Some(env) = t.env.as_ref().filter(|e| !e.is_empty()) {
        lines.push("    environment:".to_string());
        for e in env {
            lines.push(format!("      {}: ${{{}}}", e.name, e.name));
        }
    }
    if let Some(cmd) = &t.command {
        lines.push(format!("    command: {cmd}"));
    }
    if let Some(net) = &t.network {
        lines.push(format!("    network_mode: {net}"));
    }
    let named: Vec<&PortainerVolume> = volumes.iter().filter(|v| v.bind.is_none()).collect();
    if !named.is_empty() {
        lines.push("volumes:".to_string());
        for v in named {
            lines.push(format!("  {}_data:", slug(&v.container)));
        }
    }
    lines.join("\n") + "\n"
}

/// Parses a Portainer app-template document — `{ version, templates: [...] }` or a bare array —
/// into Photon template manifests. `source_name` is stamped into a compose stack's placeholder
/// compose text. An entry that cannot be mapped is reported in `skipped` with its reason, never
/// silently dropped; the whole document is rejected only when it is not JSON or holds no
/// templates at all.
pub fn parse_portainer_templates(json: &str, source_name: &str) -> Result<ImportReport, String> {
    let doc: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Not valid JSON: {e}"))?;
    let list: Vec<serde_json::Value> = match doc.get("templates").and_then(|t| t.as_array()) {
        Some(arr) => arr.clone(),
        None => doc.as_array().cloned().unwrap_or_default(),
    };
    if list.is_empty() {
        return Err(
            "No `templates` array found. Expected Portainer app-template JSON v2 or v3."
                .to_string(),
        );
    }
    let version = match doc.get("version") {
        Some(serde_json::Value::String(s)) if s == "3" => 3,
        Some(serde_json::Value::Number(n)) if n.as_u64() == Some(3) => 3,
        _ => 2,
    };
    let mut report = ImportReport {
        imported: Vec::new(),
        skipped: Vec::new(),
        version,
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for raw in list {
        let t: PortainerTemplate = match serde_json::from_value(raw) {
            Ok(t) => t,
            Err(e) => {
                report.skipped.push(Skipped {
                    title: "(unreadable)".to_string(),
                    reason: format!("not a template entry: {e}"),
                });
                continue;
            }
        };
        let Some(title) = t.title.as_deref().filter(|s| !s.is_empty()) else {
            report.skipped.push(Skipped {
                title: "(untitled)".to_string(),
                reason: "no title".to_string(),
            });
            continue;
        };
        if t.kind == 2 {
            report.skipped.push(Skipped {
                title: title.to_string(),
                reason: "Swarm stack — Photon does not cluster".to_string(),
            });
            continue;
        }
        if t.kind != 1 && t.kind != 3 {
            report.skipped.push(Skipped {
                title: title.to_string(),
                reason: format!("unsupported type {}", t.kind),
            });
            continue;
        }
        let mut id = slug(t.name.as_deref().unwrap_or(title));
        while seen.contains(&id) {
            id.push_str("-2");
        }
        seen.insert(id.clone());
        let manifest = TemplateManifest {
            id,
            version: "1.0.0".to_string(),
            name: title.to_string(),
            description: t.description.clone().unwrap_or_default(),
            icon: t.logo.clone().unwrap_or_default(),
            categories: t
                .categories
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|c| c.to_lowercase())
                .collect(),
            variables: t
                .env
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(to_variable)
                .collect(),
            requires: TemplateRequires {
                arch: t.platform.clone().map(|p| vec![p]),
                min_docker: None,
                min_ram_bytes: None,
                min_disk_bytes: None,
            },
            ports: Vec::new(),
            volumes: Vec::new(),
            health: None,
            observability: TemplateObservability::default(),
        };
        let (compose_text, source_kind) = if t.kind == 3 {
            let repo = t.repository.as_ref();
            (
                format!(
                    "# from {source_name}: {} {}\n# (fetched on sync)\n",
                    repo.and_then(|r| r.url.as_deref()).unwrap_or("?"),
                    repo.and_then(|r| r.stackfile.as_deref()).unwrap_or("")
                ),
                ImportSourceKind::ComposeStack,
            )
        } else {
            (container_compose(&t), ImportSourceKind::Container)
        };
        report.imported.push(ImportedTemplate {
            manifest,
            compose_text,
            source_kind,
        });
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V3: &str = r#"{
      "version": "3",
      "templates": [
        { "type": 3, "title": "Uptime Kuma", "description": "monitor", "categories": ["Monitoring"],
          "repository": { "url": "https://x", "stackfile": "stacks/kuma.yml" },
          "env": [{ "name": "PORT", "label": "Port", "default": "3001" }] },
        { "type": 1, "title": "Nginx", "image": "nginx:latest", "ports": ["80:80/tcp"],
          "volumes": [{ "container": "/usr/share/nginx/html" }],
          "env": [
            { "name": "MODE", "select": [{ "text": "A", "value": "a", "default": true }, { "text": "B", "value": "b" }] },
            { "name": "ADMIN_PASSWORD", "label": "Password" }
          ] },
        { "type": 2, "title": "Swarm thing" },
        { "type": 9, "title": "Weird" }
      ]
    }"#;

    #[test]
    fn imports_compose_stacks_and_containers_skips_swarm_and_unknown_types() {
        let r = parse_portainer_templates(V3, "community").unwrap();
        assert_eq!(r.version, 3);
        let ids: Vec<&str> = r.imported.iter().map(|t| t.manifest.id.as_str()).collect();
        assert_eq!(ids, vec!["uptime-kuma", "nginx"]);
        let skipped: Vec<&str> = r.skipped.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(skipped, vec!["Swarm thing", "Weird"]);
        assert_eq!(r.skipped[0].reason, "Swarm stack — Photon does not cluster");
        assert_eq!(r.skipped[1].reason, "unsupported type 9");

        let kuma = &r.imported[0];
        assert_eq!(kuma.source_kind, ImportSourceKind::ComposeStack);
        assert_eq!(
            kuma.compose_text,
            "# from community: https://x stacks/kuma.yml\n# (fetched on sync)\n"
        );
        assert_eq!(kuma.manifest.variables[0].kind, VariableType::Port);
        assert!(!kuma.manifest.variables[0].required);
        assert_eq!(kuma.manifest.variables[0].default.as_deref(), Some("3001"));
        assert_eq!(kuma.manifest.variables[0].label, "Port");
        assert_eq!(kuma.manifest.categories, vec!["monitoring"]);
        assert_eq!(kuma.manifest.description, "monitor");
        assert_eq!(kuma.manifest.name, "Uptime Kuma");
        assert_eq!(kuma.manifest.version, "1.0.0");

        let nginx = &r.imported[1];
        assert_eq!(nginx.source_kind, ImportSourceKind::Container);
        assert!(nginx.compose_text.contains("image: nginx:latest"));
        assert!(nginx.compose_text.contains("\"80:80/tcp\""));
        assert_eq!(
            nginx.compose_text,
            "services:\n  nginx:\n    image: nginx:latest\n    restart: unless-stopped\n    ports:\n      - \"80:80/tcp\"\n    volumes:\n      - usr-share-nginx-html_data:/usr/share/nginx/html\n    environment:\n      MODE: ${MODE}\n      ADMIN_PASSWORD: ${ADMIN_PASSWORD}\nvolumes:\n  usr-share-nginx-html_data:\n"
        );
        let mode = nginx
            .manifest
            .variables
            .iter()
            .find(|v| v.name == "MODE")
            .unwrap();
        assert_eq!(mode.kind, VariableType::Select);
        assert_eq!(mode.default.as_deref(), Some("a"));
        assert_eq!(
            mode.options.as_deref(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
        assert!(!mode.required);
        let pw = nginx
            .manifest
            .variables
            .iter()
            .find(|v| v.name == "ADMIN_PASSWORD")
            .unwrap();
        assert_eq!(pw.kind, VariableType::Secret);
        assert!(pw.required);
        assert_eq!(pw.label, "Password");
        assert_eq!(pw.preset, Some(false));
        // The synthesized compose is a valid stack as far as the lint and scan are concerned.
        assert_eq!(
            crate::compose::compose_services(&nginx.compose_text),
            vec!["nginx"]
        );
    }

    #[test]
    fn rejects_non_json_and_json_without_templates() {
        assert!(parse_portainer_templates("nope", "portainer")
            .unwrap_err()
            .contains("JSON"));
        assert!(parse_portainer_templates("{\"a\":1}", "portainer")
            .unwrap_err()
            .contains("templates"));
        assert!(parse_portainer_templates("[]", "portainer")
            .unwrap_err()
            .contains("templates"));
    }

    #[test]
    fn v2_bare_array_untitled_duplicates_and_binds() {
        let json = r#"[
          { "type": 1, "title": "App", "name": "app", "image": "a:1", "platform": "linux",
            "volumes": [{ "container": "/data", "bind": "/opt/photon/app", "readonly": true }],
            "restart_policy": "always", "command": "run --fast", "network": "host",
            "env": [{ "name": "DATA_DIR", "default": "/data", "preset": true }, { "name": "API_KEY", "description": "d" }] },
          { "type": 1, "title": "App" },
          { "type": 3, "title": "App", "name": "app" },
          { "type": 1 },
          { "type": "one", "title": "Stringly" }
        ]"#;
        let r = parse_portainer_templates(json, "src").unwrap();
        assert_eq!(r.version, 2);
        let ids: Vec<&str> = r.imported.iter().map(|t| t.manifest.id.as_str()).collect();
        assert_eq!(ids, vec!["app", "app-2", "app-2-2"]);
        assert_eq!(r.skipped.len(), 2);
        assert_eq!(
            r.skipped[0],
            Skipped {
                title: "(untitled)".into(),
                reason: "no title".into()
            }
        );
        assert_eq!(r.skipped[1].title, "(unreadable)");
        let app = &r.imported[0];
        assert_eq!(
            app.compose_text,
            "services:\n  app:\n    image: a:1\n    restart: always\n    volumes:\n      - /opt/photon/app:/data:ro\n    environment:\n      DATA_DIR: ${DATA_DIR}\n      API_KEY: ${API_KEY}\n    command: run --fast\n    network_mode: host\n"
        );
        assert_eq!(
            app.manifest.requires.arch.as_deref(),
            Some(&["linux".to_string()][..])
        );
        let dir = &app.manifest.variables[0];
        assert_eq!(dir.kind, VariableType::Path);
        assert_eq!(dir.preset, Some(true));
        assert!(!dir.required);
        let key = &app.manifest.variables[1];
        assert_eq!(key.kind, VariableType::Secret);
        assert_eq!(key.description.as_deref(), Some("d"));
        assert!(key.required);
        // A stack with no repository still imports, with a `?` placeholder.
        assert_eq!(
            r.imported[2].compose_text,
            "# from src: ? \n# (fetched on sync)\n"
        );
        // Every imported manifest passes the publish validation.
        for t in &r.imported {
            assert_eq!(
                crate::render::validate_manifest(&t.manifest),
                Ok(()),
                "{}",
                t.manifest.id
            );
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Uptime Kuma"), "uptime-kuma");
        assert_eq!(slug("  --Hello, World!--  "), "hello-world");
        assert_eq!(slug("/usr/share/nginx/html"), "usr-share-nginx-html");
        assert_eq!(slug("!!!"), "template");
        assert_eq!(slug(""), "template");
        assert_eq!(slug("Éclair"), "clair");
        assert_eq!(slug(&"a".repeat(60)).len(), 48);
    }
}
