//! Compose-file helpers: the dangerous-construct lint (R8.4), the service scan, `.env` parsing,
//! variable references and substitution, and stack-file validation. A faithful port of
//! `frontend/src/lib/deploy/compose.ts` (+ `mockValidateCompose` in `mockDeploy.ts`): the
//! frontend acknowledges a warning by its `id`, so the server MUST compute the same ids, the
//! same `line`, and the same `detail` wording — the editor warns while the operator types, the
//! server is the one that refuses to ship. Pure, no I/O.
//!
//! The lint is deliberately line-based rather than YAML-based, like the TS: a half-typed file
//! that does not parse still gets its `privileged: true` flagged. `compose_services` and
//! `service_images` go through serde_yaml first and fall back to the line scan only when the
//! text is not YAML.

use crate::model::{ComposeWarning, StackFile, StackValidation, WarningKind, WarningSeverity};
use std::collections::BTreeMap;

/// Bind mounts are allowed under these prefixes; anything else on the host filesystem is flagged.
pub const ALLOWED_BIND_PREFIXES: [&str; 4] = ["/opt/photon", "/var/lib/photon", "/srv", "/data"];

/// Docker's own recognised compose filenames, in lookup order (R4.2).
pub const COMPOSE_FILENAMES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// Arcane's caps (R4.4): they exist for a reason.
pub const FILE_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 2000;
pub const MAX_DEPTH: usize = 20;

pub fn is_compose_file(path: &str) -> bool {
    COMPOSE_FILENAMES.contains(&path)
}

/// A path is safe when it stays inside the stack folder: relative, no `..`, no absolute root,
/// no backslashes, at most `MAX_DEPTH` segments.
pub fn is_safe_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts
        .iter()
        .any(|p| *p == ".." || p.is_empty() || *p == ".")
    {
        return false;
    }
    parts.len() <= MAX_DEPTH
}

// ---------------------------------------------------------------------------------------------
// Line model (shared by the lint and the fallback service scan)
// ---------------------------------------------------------------------------------------------

struct Line<'a> {
    /// 1-based line number.
    n: usize,
    indent: usize,
    text: &'a str,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    text.split('\n')
        .map(|raw| raw.strip_suffix('\r').unwrap_or(raw))
        .enumerate()
        .map(|(i, raw)| Line {
            n: i + 1,
            indent: raw.len() - raw.trim_start().len(),
            text: raw.trim(),
        })
        .collect()
}

/// `^[A-Za-z0-9_.-]+:\s*$` on an already-trimmed line: a bare mapping key.
fn service_header(text: &str) -> Option<&str> {
    let name = text.strip_suffix(':')?;
    if name.is_empty() {
        return None;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        .then_some(name)
}

/// The line-based service scan the TS uses everywhere; here the fallback for non-YAML text.
fn compose_services_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_services = false;
    let mut service_indent: Option<usize> = None;
    for l in lines(text) {
        if l.text.is_empty() || l.text.starts_with('#') {
            continue;
        }
        if l.indent == 0 {
            in_services = l.text == "services:";
            service_indent = None;
            continue;
        }
        if !in_services {
            continue;
        }
        let si = *service_indent.get_or_insert(l.indent);
        if l.indent == si {
            if let Some(name) = service_header(l.text) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The service a given line belongs to, walking back to the nearest service header.
fn service_at(all: &[Line<'_>], idx: usize) -> Option<String> {
    let mut in_services = false;
    let mut service_indent: Option<usize> = None;
    let mut current: Option<String> = None;
    for l in &all[..=idx] {
        if l.text.is_empty() || l.text.starts_with('#') {
            continue;
        }
        if l.indent == 0 {
            in_services = l.text == "services:";
            service_indent = None;
            current = None;
            continue;
        }
        if !in_services {
            continue;
        }
        let si = *service_indent.get_or_insert(l.indent);
        if l.indent == si {
            if let Some(name) = service_header(l.text) {
                current = Some(name.to_string());
            }
        }
    }
    current
}

// ---------------------------------------------------------------------------------------------
// YAML view
// ---------------------------------------------------------------------------------------------

fn yaml_key(k: &serde_yaml::Value) -> Option<String> {
    match k {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The top-level `services:` mapping when the text is YAML and has one. `Err` = not YAML at all
/// (fall back to the line scan); `Ok(None)` = YAML without a services mapping.
fn yaml_services(text: &str) -> Result<Option<serde_yaml::Mapping>, ()> {
    let doc: serde_yaml::Value = serde_yaml::from_str(text).map_err(|_| ())?;
    Ok(doc.get("services").and_then(|s| s.as_mapping()).cloned())
}

/// Service names under the top-level `services:` block, in file order.
pub fn compose_services(text: &str) -> Vec<String> {
    match yaml_services(text) {
        Ok(Some(m)) => m.keys().filter_map(yaml_key).collect(),
        Ok(None) => Vec::new(),
        Err(()) => compose_services_lines(text),
    }
}

/// service → its raw `image:` string (may still carry `${VAR}` references).
pub fn service_images(text: &str) -> BTreeMap<String, String> {
    match yaml_services(text) {
        Ok(Some(m)) => m
            .iter()
            .filter_map(|(k, v)| {
                let name = yaml_key(k)?;
                let image = v.get("image")?;
                let image = match image {
                    serde_yaml::Value::String(s) => s.clone(),
                    serde_yaml::Value::Number(n) => n.to_string(),
                    _ => return None,
                };
                Some((name, image))
            })
            .collect(),
        Ok(None) => BTreeMap::new(),
        Err(()) => {
            // Not YAML (yet): the first `image:` line after each service header.
            let all = lines(text);
            let mut out = BTreeMap::new();
            for (i, l) in all.iter().enumerate() {
                if let Some(rest) = l.text.strip_prefix("image:") {
                    let raw = rest.trim();
                    let image = strip_quotes(raw);
                    if image.is_empty() {
                        continue;
                    }
                    if let Some(svc) = service_at(&all, i) {
                        out.entry(svc).or_insert_with(|| image.to_string());
                    }
                }
            }
            out
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lint
// ---------------------------------------------------------------------------------------------

/// The wire spelling of a kind — what the frontend concatenates into `id`. Kept in step with
/// `WarningKind`'s serde rename by a test below.
fn kind_str(kind: WarningKind) -> &'static str {
    match kind {
        WarningKind::Privileged => "privileged",
        WarningKind::DockerSock => "docker_sock",
        WarningKind::HostNetwork => "host_network",
        WarningKind::HostPid => "host_pid",
        WarningKind::BindMountOutside => "bind_mount_outside",
        WarningKind::CapAdd => "cap_add",
    }
}

/// `^KEY:\s*["']?host["']?\s*$` on a trimmed line (`network_mode: host`, `pid: "host"`).
fn is_host_value(text: &str, key: &str) -> bool {
    let Some(rest) = text.strip_prefix(key).and_then(|r| r.strip_prefix(':')) else {
        return false;
    };
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(['"', '\'']).unwrap_or(rest);
    let Some(rest) = rest.strip_prefix("host") else {
        return false;
    };
    let rest = rest.strip_prefix(['"', '\'']).unwrap_or(rest);
    rest.trim().is_empty()
}

/// `^privileged:\s*true\b`.
fn is_privileged(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("privileged:") else {
        return false;
    };
    let rest = rest.trim_start();
    match rest.strip_prefix("true") {
        Some(after) => !after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'),
        None => false,
    }
}

/// `.replace(/^["']|["']$/g, '')`: one quote off each end.
fn strip_quotes(s: &str) -> &str {
    let s = s.strip_prefix(['"', '\'']).unwrap_or(s);
    s.strip_suffix(['"', '\'']).unwrap_or(s)
}

/// The constructs an operator must acknowledge before a compose file ships to a customer host.
/// Each warning's `id` is `kind:service` (or `kind:-` outside any service), stable across edits
/// of the same construct so an acknowledgement sticks; a service with two docker.sock mounts is
/// one acknowledgement.
pub fn lint_compose(text: &str) -> Vec<ComposeWarning> {
    let all = lines(text);
    let mut out: Vec<ComposeWarning> = Vec::new();
    let mut push = |kind: WarningKind, severity: WarningSeverity, i: usize, detail: String| {
        let service = service_at(&all, i);
        out.push(ComposeWarning {
            id: format!("{}:{}", kind_str(kind), service.as_deref().unwrap_or("-")),
            kind,
            severity,
            service,
            line: Some(all[i].n),
            detail,
        });
    };
    let mut in_volumes_list = false;
    for (i, l) in all.iter().enumerate() {
        if l.text.is_empty() || l.text.starts_with('#') {
            continue;
        }
        if is_privileged(l.text) {
            push(
                WarningKind::Privileged,
                WarningSeverity::Error,
                i,
                "Runs as root on the host with every capability. Almost nothing needs this."
                    .to_string(),
            );
        }
        if is_host_value(l.text, "network_mode") {
            push(
                WarningKind::HostNetwork,
                WarningSeverity::Warning,
                i,
                "Shares the host's network namespace: every port the container opens is open on the host."
                    .to_string(),
            );
        }
        if is_host_value(l.text, "pid") {
            push(
                WarningKind::HostPid,
                WarningSeverity::Warning,
                i,
                "Sees and can signal every process on the host.".to_string(),
            );
        }
        if l.text.starts_with("cap_add:") {
            push(
                WarningKind::CapAdd,
                WarningSeverity::Warning,
                i,
                "Adds Linux capabilities beyond the default set.".to_string(),
            );
        }
        if l.text.contains("docker.sock") {
            push(
                WarningKind::DockerSock,
                WarningSeverity::Error,
                i,
                "Mounts the Docker socket: full control of the daemon, which is root on the host."
                    .to_string(),
            );
        }
        if l.text == "volumes:" {
            in_volumes_list = true;
        } else if l.indent == 0
            || (!l.text.starts_with('-') && in_volumes_list && !l.text.starts_with("volumes:"))
        {
            in_volumes_list = false;
        }
        if in_volumes_list && l.text.starts_with('-') {
            let spec = strip_quotes(l.text[1..].trim());
            let src = spec.split(':').next().unwrap_or("");
            let allowed = ALLOWED_BIND_PREFIXES
                .iter()
                .any(|p| src == *p || src.starts_with(&format!("{p}/")));
            if src.starts_with('/') && !spec.contains("docker.sock") && !allowed {
                push(
                    WarningKind::BindMountOutside,
                    WarningSeverity::Warning,
                    i,
                    format!(
                        "Bind-mounts {src} from the host, outside the allowed prefixes ({}).",
                        ALLOWED_BIND_PREFIXES.join(", ")
                    ),
                );
            }
        }
    }
    // De-duplicate by id, keeping the first occurrence.
    let mut seen = std::collections::HashSet::new();
    out.retain(|w| seen.insert(w.id.clone()));
    out
}

// ---------------------------------------------------------------------------------------------
// Variables
// ---------------------------------------------------------------------------------------------

/// `.env` → { KEY: value }, ignoring comments and blanks; one pair of matching quotes stripped.
pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        if eq == 0 {
            continue;
        }
        let key = line[..eq].trim();
        let mut val = line[eq + 1..].trim();
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = &val[1..val.len() - 1];
        } else if val.len() == 1 && (val == "\"" || val == "'") {
            // JS: `"`.startsWith('"') && `"`.endsWith('"') → slice(1, -1) = "".
            val = "";
        }
        out.insert(key.to_string(), val.to_string());
    }
    out
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The identifier starting at `at`, if any.
fn ident_at(bytes: &[u8], at: usize) -> Option<&str> {
    if !bytes.get(at).copied().is_some_and(is_ident_start) {
        return None;
    }
    let mut end = at + 1;
    while bytes.get(end).copied().is_some_and(is_ident_char) {
        end += 1;
    }
    std::str::from_utf8(&bytes[at..end]).ok()
}

/// One parsed `${…}` reference: the name, the modifier (`:-`, `-`, `:?`, `?`, `:+`, `+`) with its
/// word, and the byte index just past the closing `}`.
struct BraceRef<'a> {
    name: &'a str,
    modifier: Option<(&'a str, &'a str)>,
    end: usize,
}

/// Parses `${NAME}` / `${NAME<mod>word}` starting at the `$` at `at` — the regex
/// `\$\{([A-Za-z_][A-Za-z0-9_]*)(?::?[-?+][^}]*)?\}`. Modifier words may contain anything but `}`.
fn brace_ref_at(bytes: &[u8], at: usize) -> Option<BraceRef<'_>> {
    if bytes.get(at) != Some(&b'$') || bytes.get(at + 1) != Some(&b'{') {
        return None;
    }
    let name = ident_at(bytes, at + 2)?;
    let mut i = at + 2 + name.len();
    if bytes.get(i) == Some(&b'}') {
        return Some(BraceRef {
            name,
            modifier: None,
            end: i + 1,
        });
    }
    let mod_start = i;
    if bytes.get(i) == Some(&b':') {
        i += 1;
    }
    if !matches!(bytes.get(i), Some(b'-' | b'?' | b'+')) {
        return None;
    }
    i += 1;
    let word_start = i;
    while let Some(&b) = bytes.get(i) {
        if b == b'}' {
            let modifier = std::str::from_utf8(&bytes[mod_start..word_start]).ok()?;
            let word = std::str::from_utf8(&bytes[word_start..i]).ok()?;
            return Some(BraceRef {
                name,
                modifier: Some((modifier, word)),
                end: i + 1,
            });
        }
        i += 1;
    }
    None
}

/// `${VAR}` / `${VAR:-default}` / `$VAR` references in a compose file, deduplicated, in order of
/// first appearance. `$$` is Compose's escape for a literal `$` and is never a reference;
/// `${secret:NAME}` is a secret-store reference, not an env variable, and is not one either.
pub fn compose_variable_refs(text: &str) -> Vec<String> {
    // Strip `$$` pairs left to right first, exactly like the TS `replace(/\$\$/g, '')`.
    let mut unescaped: Vec<u8> = Vec::with_capacity(text.len());
    let src = text.as_bytes();
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'$' && src.get(i + 1) == Some(&b'$') {
            i += 2;
        } else {
            unescaped.push(src[i]);
            i += 1;
        }
    }
    let bytes = &unescaped[..];
    let mut refs: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if !refs.iter().any(|r| r == name) {
            refs.push(name.to_string());
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        if let Some(r) = brace_ref_at(bytes, i) {
            add(r.name);
            i = r.end;
        } else if let Some(name) = ident_at(bytes, i + 1) {
            add(name);
            i += 1 + name.len();
        } else {
            i += 1;
        }
    }
    refs
}

/// Variables the compose file references that neither `env` nor `known` (the secret store,
/// template variables, …) provides. A reference with a default (`${X:-v}`) still counts as a
/// reference, as in the TS.
pub fn missing_variables(
    compose: &str,
    env: &BTreeMap<String, String>,
    known: &[String],
) -> Vec<String> {
    compose_variable_refs(compose)
        .into_iter()
        .filter(|v| !env.contains_key(v) && !known.iter().any(|k| k == v))
        .collect()
}

/// Replaces `${NAME}` with `values[NAME]` when present and non-empty, and `${NAME:-default}` /
/// `${NAME-default}` with the value or else the default. Every other reference — an unknown
/// name, a bare `$NAME`, an `:?`/`:+` modifier without a value — is left untouched for the
/// agent's `docker compose` to resolve against the rendered `.env`, and `$$` is left as the
/// escape it is. Byte-exact everywhere else.
pub fn substitute(text: &str, values: &BTreeMap<String, String>) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut literal_from = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        if bytes.get(i + 1) == Some(&b'$') {
            i += 2;
            continue;
        }
        let Some(r) = brace_ref_at(bytes, i) else {
            i += 1;
            continue;
        };
        let value = values.get(r.name).filter(|v| !v.is_empty());
        let replacement: Option<&str> = match (value, r.modifier) {
            (Some(v), _) => Some(v.as_str()),
            (None, Some((":-" | "-", word))) => Some(word),
            (None, _) => None,
        };
        if let Some(rep) = replacement {
            out.push_str(&text[literal_from..i]);
            out.push_str(rep);
            literal_from = r.end;
        }
        i = r.end;
    }
    out.push_str(&text[literal_from..]);
    out
}

// ---------------------------------------------------------------------------------------------
// Stack validation
// ---------------------------------------------------------------------------------------------

fn invalid(error: String) -> StackValidation {
    StackValidation {
        ok: false,
        warnings: Vec::new(),
        services: Vec::new(),
        missing_variables: Vec::new(),
        error: Some(error),
    }
}

/// What `POST /api/stacks/validate` answers: the compose file found, every path inside the
/// folder, the caps respected, at least one service, plus the lint warnings and the variables
/// the compose references that `.env` does not provide (`${secret:…}` references excluded).
pub fn validate_stack_files(files: &[StackFile]) -> StackValidation {
    if files.len() > MAX_ENTRIES {
        return invalid(format!(
            "Too many files: {} (the limit is {MAX_ENTRIES}).",
            files.len()
        ));
    }
    if let Some(big) = files.iter().find(|f| f.content.len() > FILE_MAX_BYTES) {
        return invalid(format!(
            "\"{}\" is {} bytes; the limit per file is 10 MiB.",
            big.path,
            big.content.len()
        ));
    }
    let Some(compose) = files.iter().find(|f| is_compose_file(&f.path)) else {
        return invalid(format!(
            "No compose file. Expected one of {}.",
            COMPOSE_FILENAMES.join(", ")
        ));
    };
    if let Some(bad) = files.iter().find(|f| !is_safe_path(&f.path)) {
        return invalid(format!(
            "\"{}\" escapes the stack folder. Paths must be relative and stay inside it.",
            bad.path
        ));
    }
    let services = compose_services(&compose.content);
    if services.is_empty() {
        return invalid("No services found under `services:`.".to_string());
    }
    let env = files
        .iter()
        .find(|f| f.path == ".env")
        .map(|f| parse_env(&f.content))
        .unwrap_or_default();
    let missing: Vec<String> = missing_variables(&compose.content, &env, &[])
        .into_iter()
        .filter(|v| !v.starts_with("secret:"))
        .collect();
    StackValidation {
        ok: true,
        warnings: lint_compose(&compose.content),
        services,
        missing_variables: missing,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPOSE: &str = "services:
  api:
    image: x:1
    privileged: true
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - /etc/ssl:/certs:ro
      - /opt/photon/data:/data
      - app:/var/lib/app
  worker:
    image: y:${VERSION}
    network_mode: host
    environment:
      TOKEN: ${secret:TOKEN}
      LEVEL: ${LOG_LEVEL:-info}
volumes:
  app:
";

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn lists_services_under_services_only() {
        assert_eq!(compose_services(COMPOSE), vec!["api", "worker"]);
        assert_eq!(compose_services("volumes:\n  a:\n"), Vec::<String>::new());
        assert_eq!(compose_services(""), Vec::<String>::new());
    }

    #[test]
    fn service_scan_falls_back_to_lines_when_not_yaml() {
        // A half-typed file: unbalanced quote makes it invalid YAML, the line scan still sees it.
        let text = "services:\n  api:\n    image: \"x\n  worker:\n    image: y\n";
        assert!(serde_yaml::from_str::<serde_yaml::Value>(text).is_err());
        assert_eq!(compose_services(text), vec!["api", "worker"]);
        assert_eq!(compose_services_lines(COMPOSE), vec!["api", "worker"]);
    }

    #[test]
    fn flags_the_dangerous_constructs_per_service_once_each_with_stable_ids() {
        let w = lint_compose(COMPOSE);
        let ids: Vec<&str> = w.iter().map(|x| x.id.as_str()).collect();
        assert!(ids.contains(&"privileged:api"));
        assert!(ids.contains(&"docker_sock:api"));
        assert!(ids.contains(&"bind_mount_outside:api"));
        assert!(ids.contains(&"host_network:worker"));
        assert_eq!(
            ids.iter()
                .filter(|i| **i == "bind_mount_outside:api")
                .count(),
            1
        );
        let find = |id: &str| w.iter().find(|x| x.id == id).unwrap();
        assert_eq!(find("privileged:api").severity, WarningSeverity::Error);
        assert_eq!(
            find("host_network:worker").severity,
            WarningSeverity::Warning
        );
        assert!(find("bind_mount_outside:api").detail.contains("/etc/ssl"));
        assert_eq!(
            find("bind_mount_outside:api").detail,
            "Bind-mounts /etc/ssl from the host, outside the allowed prefixes (/opt/photon, /var/lib/photon, /srv, /data)."
        );
        // Lines are 1-based positions of the construct in the text.
        assert_eq!(find("privileged:api").line, Some(4));
        assert_eq!(find("docker_sock:api").line, Some(6));
        assert_eq!(find("bind_mount_outside:api").line, Some(7));
        assert_eq!(find("host_network:worker").line, Some(12));
        assert_eq!(
            find("host_network:worker").service.as_deref(),
            Some("worker")
        );
        assert_eq!(find("privileged:api").kind, WarningKind::Privileged);
        // /opt/photon and named volumes are fine
        assert_eq!(
            lint_compose(
                "services:\n  a:\n    image: x\n    volumes:\n      - /opt/photon/x:/x\n      - data:/d\n"
            ),
            Vec::new()
        );
    }

    #[test]
    fn lint_covers_pid_cap_add_and_quoted_host_values() {
        let text = "services:\n  a:\n    pid: \"host\"\n    cap_add:\n      - NET_ADMIN\n  b:\n    network_mode: 'host'\n";
        let ids: Vec<String> = lint_compose(text).into_iter().map(|w| w.id).collect();
        assert_eq!(ids, vec!["host_pid:a", "cap_add:a", "host_network:b"]);
        // `network_mode: hostx` and `privileged: trueish` are not matches.
        assert!(lint_compose(
            "services:\n  a:\n    network_mode: hostx\n    privileged: trueish\n"
        )
        .is_empty());
        // Outside any service the id falls back to `-`.
        assert_eq!(lint_compose("privileged: true\n")[0].id, "privileged:-");
    }

    #[test]
    fn kind_str_matches_the_serde_spelling() {
        for k in [
            WarningKind::Privileged,
            WarningKind::DockerSock,
            WarningKind::HostNetwork,
            WarningKind::HostPid,
            WarningKind::BindMountOutside,
            WarningKind::CapAdd,
        ] {
            let json = serde_json::to_string(&k).unwrap();
            assert_eq!(json, format!("\"{}\"", kind_str(k)));
        }
    }

    #[test]
    fn parses_env_and_finds_unresolved_references() {
        assert_eq!(
            parse_env("# c\nA=1\nB=\"two words\"\nbad\nC='q'\n"),
            env(&[("A", "1"), ("B", "two words"), ("C", "q")])
        );
        assert_eq!(compose_variable_refs(COMPOSE), vec!["VERSION", "LOG_LEVEL"]);
        assert_eq!(
            missing_variables(COMPOSE, &env(&[("VERSION", "1")]), &[]),
            vec!["LOG_LEVEL"]
        );
        assert_eq!(
            missing_variables(
                COMPOSE,
                &env(&[("VERSION", "1"), ("LOG_LEVEL", "debug")]),
                &[]
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            missing_variables(
                COMPOSE,
                &env(&[("VERSION", "1")]),
                &["LOG_LEVEL".to_string()]
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn variable_refs_handle_every_modifier_and_the_dollar_escape() {
        let text = "a: $A\nb: ${B}\nc: ${C:-x}\nd: ${D-x}\ne: ${E:?err}\nf: ${F?err}\ng: ${G:+alt}\nh: ${H+alt}\nlit: $$NOT\nlit2: $${NOT2}\nnum: $1\nbad: ${bad-name}\nagain: ${B}\n";
        // `${bad-name}` is `bad` with a `-name` default — the TS regex reads it the same way.
        assert_eq!(
            compose_variable_refs(text),
            vec!["A", "B", "C", "D", "E", "F", "G", "H", "bad"]
        );
        assert_eq!(
            compose_variable_refs("${secret:TOKEN}"),
            Vec::<String>::new()
        );
        assert_eq!(compose_variable_refs("héllo ${Ünicode} $X"), vec!["X"]);
    }

    #[test]
    fn substitutes_known_references_and_leaves_the_rest() {
        let values = env(&[("VERSION", "2"), ("EMPTY", "")]);
        assert_eq!(
            substitute(
                "image: y:${VERSION}\nx: ${OTHER}\nd: ${LOG_LEVEL:-info}\ne: ${VERSION:-9}\n",
                &values
            ),
            "image: y:2\nx: ${OTHER}\nd: info\ne: 2\n"
        );
        // Empty values do not substitute; `$$` and bare `$VAR` are left alone.
        assert_eq!(
            substitute("${EMPTY} $${VERSION} $VERSION ${EMPTY-dflt}", &values),
            "${EMPTY} $${VERSION} $VERSION dflt"
        );
        // Other modifiers only substitute when the value is present.
        assert_eq!(
            substitute("${VERSION:?e} ${MISSING:?e} ${VERSION:+alt}", &values),
            "2 ${MISSING:?e} 2"
        );
        assert_eq!(substitute("héllo ${VERSION}", &values), "héllo 2");
    }

    #[test]
    fn service_images_by_service() {
        let imgs = service_images(COMPOSE);
        assert_eq!(imgs.get("api").map(String::as_str), Some("x:1"));
        assert_eq!(imgs.get("worker").map(String::as_str), Some("y:${VERSION}"));
        let fallback =
            service_images("services:\n  a:\n    image: \"x\n  b:\n    image: nginx:1\n");
        assert_eq!(fallback.get("b").map(String::as_str), Some("nginx:1"));
        assert!(service_images("volumes:\n  a:\n").is_empty());
    }

    #[test]
    fn keeps_paths_inside_the_stack_folder() {
        assert!(is_safe_path("compose.yaml"));
        assert!(is_safe_path("conf/nginx.conf"));
        assert!(!is_safe_path("../etc/passwd"));
        assert!(!is_safe_path("/etc/passwd"));
        assert!(!is_safe_path("a//b"));
        assert!(!is_safe_path("./a"));
        assert!(!is_safe_path("a\\b"));
        assert!(!is_safe_path(""));
        assert!(is_safe_path(&vec!["d"; MAX_DEPTH].join("/")));
        assert!(!is_safe_path(&vec!["d"; MAX_DEPTH + 1].join("/")));
        assert!(is_compose_file("docker-compose.yml"));
        assert!(!is_compose_file("stack.yaml"));
    }

    fn file(path: &str, content: &str) -> StackFile {
        StackFile {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn validates_stack_files_like_the_mock() {
        let v = validate_stack_files(&[file("stack.yaml", COMPOSE)]);
        assert!(!v.ok);
        assert_eq!(
            v.error.as_deref(),
            Some("No compose file. Expected one of compose.yaml, compose.yml, docker-compose.yaml, docker-compose.yml.")
        );

        let v = validate_stack_files(&[file("compose.yaml", COMPOSE), file("../x", "")]);
        assert_eq!(
            v.error.as_deref(),
            Some("\"../x\" escapes the stack folder. Paths must be relative and stay inside it.")
        );

        let v = validate_stack_files(&[file("compose.yml", "volumes:\n  a:\n")]);
        assert_eq!(
            v.error.as_deref(),
            Some("No services found under `services:`.")
        );

        let v = validate_stack_files(&[file("compose.yaml", COMPOSE), file(".env", "VERSION=1\n")]);
        assert!(v.ok);
        assert_eq!(v.error, None);
        assert_eq!(v.services, vec!["api", "worker"]);
        assert_eq!(v.missing_variables, vec!["LOG_LEVEL"]);
        assert!(v.warnings.iter().any(|w| w.id == "privileged:api"));
    }

    #[test]
    fn validation_enforces_the_caps() {
        let many: Vec<StackFile> = (0..=MAX_ENTRIES)
            .map(|i| file(&format!("f{i}"), ""))
            .collect();
        let v = validate_stack_files(&many);
        assert!(v.error.unwrap().starts_with("Too many files: 2001"));

        let huge = "x".repeat(FILE_MAX_BYTES + 1);
        let v = validate_stack_files(&[file("compose.yaml", COMPOSE), file("big.bin", &huge)]);
        assert!(v.error.unwrap().starts_with("\"big.bin\" is"));
    }
}
