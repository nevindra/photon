//! Image registry credentials, the pure part: which registry an image reference names, how a
//! registry name is spelled canonically, and which stored credential a pull should use. Compiled
//! into the agent as well as the server (no `domain` feature), because both sides answer the
//! same question — the server to say so in preflight, the agent to hand the right credential to
//! `docker compose` and to the Engine API's `X-Registry-Auth` header.
//!
//! Docker's own rule for the first path component of a reference: it is a registry host when it
//! contains a `.` or a `:`, or is exactly `localhost`; otherwise the whole reference is a Docker
//! Hub repository (`nginx`, `library/nginx`, `nodefluxio/lenz`).

use crate::model::RegistryCredential;
use crate::proto::RegistryAuth;

/// Docker Hub, as an image reference names it.
pub const DOCKER_HUB: &str = "docker.io";
/// Docker Hub, as `config.json` and the Engine API's `serveraddress` spell it — the one
/// registry whose key is not its host name.
pub const DOCKER_HUB_CONFIG_KEY: &str = "https://index.docker.io/v1/";

/// The registry host an image reference pulls from: `registry.gitlab.com/nodefluxio/lenz:1.2`
/// → `registry.gitlab.com`; `nginx` / `library/nginx` / `docker.io/nginx` → `docker.io`.
pub fn image_registry(image: &str) -> String {
    let image = image.trim();
    let first = image.split('/').next().unwrap_or("");
    let is_host =
        image.contains('/') && (first.contains('.') || first.contains(':') || first == "localhost");
    if is_host {
        normalize_server(first)
    } else {
        DOCKER_HUB.to_string()
    }
}

/// A registry as an operator may type it, reduced to the host an image reference carries:
/// scheme and trailing path dropped, lower-cased, Docker Hub's aliases folded to `docker.io`.
pub fn normalize_server(server: &str) -> String {
    let s = server.trim();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let s = s.split('/').next().unwrap_or("").trim_end_matches('/');
    let s = s.to_ascii_lowercase();
    match s.as_str() {
        "index.docker.io"
        | "registry-1.docker.io"
        | "registry.hub.docker.com"
        | "hub.docker.com" => DOCKER_HUB.to_string(),
        _ => s,
    }
}

/// The `auths` key (and the `serveraddress`) Docker expects for a normalized registry.
pub fn config_key(server: &str) -> String {
    if server == DOCKER_HUB {
        DOCKER_HUB_CONFIG_KEY.to_string()
    } else {
        server.to_string()
    }
}

/// The credential to pull `image` with, if any — matched on the registry host.
pub fn auth_for_image<'a>(image: &str, auths: &'a [RegistryAuth]) -> Option<&'a RegistryAuth> {
    let host = image_registry(image);
    auths.iter().find(|a| normalize_server(&a.server) == host)
}

/// The stored credential a registry host resolves to, if any (server-side twin of
/// `auth_for_image`, for preflight).
pub fn credential_for_registry<'a>(
    server: &str,
    creds: &'a [RegistryCredential],
) -> Option<&'a RegistryCredential> {
    let host = normalize_server(server);
    creds.iter().find(|c| normalize_server(&c.server) == host)
}

/// The image references a set of compose images pull from, deduplicated, in first-seen order.
pub fn registries_of<'a>(images: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in images {
        let r = image_registry(i);
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_of_a_reference_follows_dockers_rule() {
        assert_eq!(image_registry("nginx"), "docker.io");
        assert_eq!(image_registry("nginx:1.27"), "docker.io");
        assert_eq!(image_registry("library/nginx"), "docker.io");
        assert_eq!(image_registry("nodefluxio/lenz:1.0"), "docker.io");
        assert_eq!(image_registry("docker.io/nodefluxio/lenz"), "docker.io");
        assert_eq!(
            image_registry("registry.gitlab.com/nodefluxio/lenz:latest"),
            "registry.gitlab.com"
        );
        assert_eq!(image_registry("ghcr.io/org/app@sha256:abc"), "ghcr.io");
        assert_eq!(image_registry("localhost:5000/app"), "localhost:5000");
        assert_eq!(image_registry("10.0.0.2:5000/app"), "10.0.0.2:5000");
        // A port-less, dot-less first component is a Hub namespace, not a host.
        assert_eq!(image_registry("myregistry/app"), "docker.io");
    }

    #[test]
    fn servers_normalize_to_the_host_an_image_names() {
        assert_eq!(
            normalize_server("https://registry.gitlab.com/"),
            "registry.gitlab.com"
        );
        assert_eq!(
            normalize_server("Registry.GitLab.com/v2/"),
            "registry.gitlab.com"
        );
        assert_eq!(normalize_server("index.docker.io"), "docker.io");
        assert_eq!(normalize_server("https://index.docker.io/v1/"), "docker.io");
        assert_eq!(normalize_server("localhost:5000"), "localhost:5000");
        assert_eq!(config_key("docker.io"), DOCKER_HUB_CONFIG_KEY);
        assert_eq!(config_key("ghcr.io"), "ghcr.io");
    }

    #[test]
    fn a_pull_picks_the_credential_of_its_registry() {
        let auths = vec![
            RegistryAuth {
                server: "https://registry.gitlab.com".into(),
                username: "deploy".into(),
                secret: "glpat".into(),
            },
            RegistryAuth {
                server: "docker.io".into(),
                username: "hub".into(),
                secret: "pw".into(),
            },
        ];
        assert_eq!(
            auth_for_image("registry.gitlab.com/nodefluxio/lenz", &auths)
                .map(|a| a.username.as_str()),
            Some("deploy")
        );
        assert_eq!(
            auth_for_image("nginx", &auths).map(|a| a.username.as_str()),
            Some("hub")
        );
        assert!(auth_for_image("ghcr.io/x/y", &auths).is_none());
        assert_eq!(
            registries_of(["nginx", "registry.gitlab.com/a/b", "postgres:16"]),
            vec!["docker.io", "registry.gitlab.com"]
        );
    }
}
