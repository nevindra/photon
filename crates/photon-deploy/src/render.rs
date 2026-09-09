//! Templates, server side: manifest validation, the deploy form's default values, validation of
//! an operator's values against a manifest, the render (compose + `.env`), version arithmetic
//! and the manifest's YAML preview. A port of `frontend/src/lib/deploy/templates.ts` — the
//! frontend runs the same rules to warn early, the server runs them to refuse. Pure, no I/O.
//!
//! Secrets never enter a file Photon keeps: the render writes a secret or generated variable as
//! a bare `KEY=` placeholder line that the AGENT fills from the secret store at apply time, and
//! substitutes only the non-secret values into the compose text.

use crate::compose;
use crate::model::{TemplateManifest, TemplateVariable, VariableType};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------------------------
// Manifest validation
// ---------------------------------------------------------------------------------------------

/// `^[a-z0-9][a-z0-9_-]*$` — a template id (also a compose project name candidate).
pub fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
}

/// `^[A-Z][A-Z0-9_]*$` — a variable name, as an env key.
pub fn is_variable_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Semver-ish: `MAJOR[.MINOR[.PATCH…]]` with an optional `-pre`/`+build` tail — digits lead,
/// every char is alphanumeric or `.`, `-`, `+`, and no empty dotted part.
pub fn is_version(s: &str) -> bool {
    let core_end = s.find(['-', '+']).unwrap_or(s.len());
    let (core, tail) = s.split_at(core_end);
    !core.is_empty()
        && core
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && tail.len() != 1
        && tail
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

/// The manifest a publish must satisfy. The first problem found is the error.
pub fn validate_manifest(m: &TemplateManifest) -> Result<(), String> {
    if !is_slug(&m.id) {
        return Err(format!(
            "id \"{}\" must be lowercase letters, digits, - or _ and start with a letter or digit",
            m.id
        ));
    }
    if !is_version(&m.version) {
        return Err(format!(
            "version \"{}\" is not a version like 1.2.0",
            m.version
        ));
    }
    if m.name.trim().is_empty() {
        return Err("name is required".to_string());
    }
    let mut seen = BTreeSet::new();
    for v in &m.variables {
        if !is_variable_name(&v.name) {
            return Err(format!(
                "variable \"{}\" must be an env-style name: uppercase letters, digits and _",
                v.name
            ));
        }
        if !seen.insert(v.name.as_str()) {
            return Err(format!("variable \"{}\" is declared twice", v.name));
        }
        if v.kind == VariableType::Select && v.options.as_ref().is_none_or(|o| o.is_empty()) {
            return Err(format!("select variable \"{}\" has no options", v.name));
        }
        if let Some(p) = &v.pattern {
            if p.is_empty() {
                return Err(format!("variable \"{}\" has an empty pattern", v.name));
            }
            if pattern::compile(p).is_err() {
                return Err(format!(
                    "variable \"{}\" has a pattern Photon cannot evaluate: {p}",
                    v.name
                ));
            }
        }
    }
    let mut port_names = BTreeSet::new();
    for p in &m.ports {
        if p.name.trim().is_empty() {
            return Err("a port needs a name".to_string());
        }
        if !port_names.insert(p.name.as_str()) {
            return Err(format!("port \"{}\" is declared twice", p.name));
        }
        if p.container == 0 || p.default_host == 0 {
            return Err(format!("port \"{}\" must be between 1 and 65535", p.name));
        }
        if p.protocol != "tcp" && p.protocol != "udp" {
            return Err(format!(
                "port \"{}\" protocol must be tcp or udp, not \"{}\"",
                p.name, p.protocol
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Variables
// ---------------------------------------------------------------------------------------------

fn is_secret_like(kind: VariableType) -> bool {
    matches!(kind, VariableType::Secret | VariableType::Generated)
}

/// The starting values for a deploy form: defaults, presets, and nothing for secrets.
///
/// A preset is pinned by THIS version: on an upgrade the stack's stored value (the old
/// version's pin) must not beat it, or the diff shows nothing and the old images ship.
pub fn default_variables(
    m: &TemplateManifest,
    existing: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for v in &m.variables {
        if is_secret_like(v.kind) {
            continue;
        }
        let value = if v.preset == Some(true) {
            v.default.clone().unwrap_or_default()
        } else if let Some(e) = existing.get(&v.name) {
            e.clone()
        } else if let Some(d) = &v.default {
            d.clone()
        } else {
            match v.kind {
                VariableType::Bool => "false".to_string(),
                VariableType::Select => v
                    .options
                    .as_ref()
                    .and_then(|o| o.first())
                    .cloned()
                    .unwrap_or_default(),
                _ => String::new(),
            }
        };
        out.insert(v.name.clone(), value);
    }
    out
}

fn is_number(s: &str) -> bool {
    // `^-?\d+(\.\d+)?$`
    let s = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// The error for one variable's value, or `None` when it passes.
pub fn validate_variable(v: &TemplateVariable, raw: Option<&str>) -> Option<String> {
    let trimmed = raw.unwrap_or("").trim();
    let value: String = if trimmed.is_empty() {
        if v.preset == Some(true) {
            v.default.clone().unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        trimmed.to_string()
    };
    if v.kind == VariableType::Generated {
        return None;
    }
    if v.kind == VariableType::Secret {
        return (v.required && value.is_empty())
            .then(|| "Required — enter it once; it is stored encrypted.".to_string());
    }
    if value.is_empty() {
        return v.required.then(|| "Required.".to_string());
    }
    match v.kind {
        VariableType::Number if !is_number(&value) => {
            return Some("Must be a number.".to_string());
        }
        VariableType::Port => {
            let n = value.parse::<u64>().ok();
            let all_digits = value.bytes().all(|b| b.is_ascii_digit());
            if !all_digits || !n.is_some_and(|n| (1..=65535).contains(&n)) {
                return Some("Must be a port between 1 and 65535.".to_string());
            }
        }
        VariableType::Bool if value != "true" && value != "false" => {
            return Some("Must be true or false.".to_string());
        }
        VariableType::Select => {
            if let Some(options) = &v.options {
                if !options.contains(&value) {
                    return Some(format!("Must be one of {}.", options.join(", ")));
                }
            }
        }
        VariableType::Path if !value.starts_with('/') => {
            return Some("Must be an absolute path on the host.".to_string());
        }
        _ => {}
    }
    if let Some(p) = &v.pattern {
        // A pattern Photon cannot evaluate must not block deploys (the TS swallows a bad RegExp).
        if let Some(false) = pattern::matches(p, &value) {
            return Some(format!("Must match {p}."));
        }
    }
    None
}

/// name → error for every variable that fails; an empty map means the deploy may proceed. A
/// secret the store already holds (`provided_secrets`) is not asked for again.
pub fn validate_variables(
    m: &TemplateManifest,
    values: &BTreeMap<String, String>,
    provided_secrets: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut errors = BTreeMap::new();
    for v in &m.variables {
        if v.kind == VariableType::Secret && provided_secrets.contains(&v.name) {
            continue;
        }
        if let Some(err) = validate_variable(v, values.get(&v.name).map(String::as_str)) {
            errors.insert(v.name.clone(), err);
        }
    }
    errors
}

// ---------------------------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------------------------

pub struct RenderInput<'a> {
    pub manifest: &'a TemplateManifest,
    pub compose_text: &'a str,
    pub values: &'a BTreeMap<String, String>,
    /// This Photon's OTLP endpoint as the host reaches it (or the one the stack already uses).
    pub otlp_endpoint: Option<&'a str>,
    pub ingest_token: Option<&'a str>,
    pub environment: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub compose: String,
    pub env: String,
    /// `secret` variables — the agent fills their `.env` placeholders from the secret store.
    pub secret_names: Vec<String>,
    /// `generated` variables — minted once at first deploy, then kept as secrets.
    pub generated_names: Vec<String>,
    /// The compose services the stack produces, for the APM tie-in.
    pub apm_services: Vec<String>,
}

/// The `.env` a template deploy renders: non-secret variables as `KEY=value`, secrets and
/// generated values as bare `KEY=` placeholders, then the observability block.
fn render_env(input: &RenderInput<'_>) -> String {
    let m = input.manifest;
    let mut lines: Vec<String> = Vec::new();
    for v in &m.variables {
        if is_secret_like(v.kind) {
            continue;
        }
        let value = input
            .values
            .get(&v.name)
            .cloned()
            .or_else(|| v.default.clone())
            .unwrap_or_default();
        lines.push(format!("{}={value}", v.name));
    }
    for v in &m.variables {
        if is_secret_like(v.kind) {
            lines.push(format!("{}=", v.name));
        }
    }
    let obs = &m.observability;
    // Both spellings: the OTel SDK convention any stock exporter reads, and the plain
    // `PHOTON_*` pair our own images (lenz, liodra, …) take, so a template's compose can say
    // `PHOTON_OTLP_TOKEN: ${PHOTON_OTLP_TOKEN}` without a per-template workaround.
    if obs.otlp_endpoint {
        if let Some(ep) = input.otlp_endpoint {
            lines.push(format!("OTEL_EXPORTER_OTLP_ENDPOINT={ep}"));
            lines.push(format!("PHOTON_OTLP_HTTP_ENDPOINT={ep}"));
        }
    }
    if obs.ingest_token {
        if let Some(tok) = input.ingest_token {
            lines.push(format!(
                "OTEL_EXPORTER_OTLP_HEADERS=Authorization=Bearer {tok}"
            ));
            lines.push(format!("PHOTON_OTLP_TOKEN={tok}"));
        }
    }
    if let Some(name) = obs.service_name.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("OTEL_SERVICE_NAME={name}"));
    }
    if obs.environment {
        if let Some(env) = input.environment.filter(|e| !e.is_empty()) {
            lines.push(format!("DEPLOYMENT_ENVIRONMENT={env}"));
            lines.push(format!(
                "OTEL_RESOURCE_ATTRIBUTES=deployment.environment={env}"
            ));
        }
    }
    lines.join("\n") + "\n"
}

/// Renders a template version for one host: the compose text with the non-secret values
/// substituted (the OTLP endpoint included when the Photon block asks for it), the `.env`, and
/// the names the agent must resolve.
pub fn render(input: RenderInput<'_>) -> Rendered {
    let m = input.manifest;
    let secret_names: Vec<String> = m
        .variables
        .iter()
        .filter(|v| v.kind == VariableType::Secret)
        .map(|v| v.name.clone())
        .collect();
    let generated_names: Vec<String> = m
        .variables
        .iter()
        .filter(|v| v.kind == VariableType::Generated)
        .map(|v| v.name.clone())
        .collect();
    let mut values: BTreeMap<String, String> = input
        .values
        .iter()
        .filter(|(k, _)| !secret_names.contains(k) && !generated_names.contains(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for v in &m.variables {
        if is_secret_like(v.kind) {
            continue;
        }
        if let (false, Some(d)) = (values.contains_key(&v.name), &v.default) {
            values.insert(v.name.clone(), d.clone());
        }
    }
    if m.observability.otlp_endpoint {
        if let Some(ep) = input.otlp_endpoint {
            values.insert("OTEL_EXPORTER_OTLP_ENDPOINT".to_string(), ep.to_string());
        }
    }
    let compose_out = compose::substitute(input.compose_text, &values);
    let apm_services = compose::compose_services(&compose_out);
    Rendered {
        env: render_env(&input),
        compose: compose_out,
        secret_names,
        generated_names,
        apm_services,
    }
}

/// A value for a `generated` variable: 32 hex chars of OS entropy, minted once at first deploy.
pub fn generate_secret() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// ---------------------------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------------------------

/// JS `Number(part)`: `""` is 0, a decimal/float literal is itself, anything else is NaN (None).
fn js_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// "1.10.0" > "1.9.2"; parts split on `.` and `-`, missing parts read as 0, non-numeric parts
/// compare as strings.
pub fn semver_compare(a: &str, b: &str) -> Ordering {
    let pa: Vec<&str> = a.split(['.', '-']).collect();
    let pb: Vec<&str> = b.split(['.', '-']).collect();
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i).copied().unwrap_or("0");
        let y = pb.get(i).copied().unwrap_or("0");
        let ord = match (js_number(x), js_number(y)) {
            (Some(nx), Some(ny)) => nx.partial_cmp(&ny).unwrap_or(Ordering::Equal),
            _ => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

/// Bump the last numeric part: "2.4.1" → "2.4.2", "1.2.0-rc1" → "1.2.0-rc2"; no digits → "v.1".
pub fn next_version(v: &str) -> String {
    let bytes = v.as_bytes();
    let Some(last_digit) = bytes.iter().rposition(|b| b.is_ascii_digit()) else {
        return format!("{v}.1");
    };
    let mut start = last_digit;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let Ok(n) = v[start..=last_digit].parse::<u64>() else {
        return format!("{v}.1");
    };
    format!("{}{}{}", &v[..start], n + 1, &v[last_digit + 1..])
}

// ---------------------------------------------------------------------------------------------
// YAML preview
// ---------------------------------------------------------------------------------------------

/// Bare when it is a plain word that YAML would not reinterpret; JSON-quoted otherwise.
fn yaml_str(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '-'));
    let reserved = matches!(
        s.to_ascii_lowercase().as_str(),
        "true" | "false" | "null" | "yes" | "no"
    );
    let leading_digit = s.chars().next().is_some_and(|c| c.is_ascii_digit());
    if plain && !reserved && !leading_digit {
        s.to_string()
    } else {
        serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
    }
}

fn flow_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| yaml_str(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// `photon.template.yaml` for the publish preview and the Compose tab — the exact shape the
/// frontend renders (`manifestToYaml`), so both sides show one document.
pub fn manifest_to_yaml(m: &TemplateManifest) -> String {
    let mut out: Vec<String> = Vec::new();
    out.push(format!("id: {}", yaml_str(&m.id)));
    out.push(format!("version: {}", yaml_str(&m.version)));
    out.push(format!("name: {}", yaml_str(&m.name)));
    out.push(format!("description: {}", yaml_str(&m.description)));
    out.push(format!("icon: {}", yaml_str(&m.icon)));
    out.push(format!("categories: {}", flow_list(&m.categories)));
    out.push("variables:".to_string());
    for v in &m.variables {
        out.push(format!("  - name: {}", v.name));
        out.push(format!("    type: {}", variable_type_str(v.kind)));
        out.push(format!("    label: {}", yaml_str(&v.label)));
        if let Some(d) = v.description.as_deref().filter(|s| !s.is_empty()) {
            out.push(format!("    description: {}", yaml_str(d)));
        }
        if v.required {
            out.push("    required: true".to_string());
        }
        if let Some(d) = v.default.as_deref().filter(|s| !s.is_empty()) {
            out.push(format!("    default: {}", yaml_str(d)));
        }
        if v.preset == Some(true) {
            out.push("    preset: true".to_string());
        }
        if let Some(o) = v.options.as_ref().filter(|o| !o.is_empty()) {
            out.push(format!("    options: {}", flow_list(o)));
        }
        if let Some(p) = v.pattern.as_deref().filter(|s| !s.is_empty()) {
            out.push(format!(
                "    pattern: {}",
                serde_json::to_string(p).unwrap_or_default()
            ));
        }
        if let Some(g) = v.group.as_deref().filter(|s| !s.is_empty()) {
            out.push(format!("    group: {}", yaml_str(g)));
        }
    }
    let r = &m.requires;
    let arch = r.arch.as_ref().filter(|a| !a.is_empty());
    let ram = r.min_ram_bytes.filter(|n| *n > 0);
    let disk = r.min_disk_bytes.filter(|n| *n > 0);
    let docker = r.min_docker.as_deref().filter(|s| !s.is_empty());
    if arch.is_some() || docker.is_some() || ram.is_some() || disk.is_some() {
        out.push("requires:".to_string());
        if let Some(a) = arch {
            out.push(format!("  arch: [{}]", a.join(", ")));
        }
        if let Some(d) = docker {
            out.push(format!("  min_docker: {}", yaml_str(d)));
        }
        if let Some(n) = ram {
            out.push(format!("  min_ram_bytes: {n}"));
        }
        if let Some(n) = disk {
            out.push(format!("  min_disk_bytes: {n}"));
        }
    }
    if !m.ports.is_empty() {
        out.push("ports:".to_string());
        for p in &m.ports {
            out.push(format!("  - name: {}", p.name));
            out.push(format!("    container: {}", p.container));
            out.push(format!("    default_host: {}", p.default_host));
            out.push(format!("    protocol: {}", p.protocol));
        }
    }
    if !m.volumes.is_empty() {
        out.push("volumes:".to_string());
        for v in &m.volumes {
            out.push(format!("  - name: {}", v.name));
            out.push(format!("    path: {}", yaml_str(&v.path)));
        }
    }
    if let Some(h) = &m.health {
        out.push("health:".to_string());
        let kind = serde_json::to_value(h.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        out.push(format!("  kind: {kind}"));
        out.push(format!("  detail: {}", yaml_str(&h.detail)));
        out.push(format!("  grace_secs: {}", h.grace_secs));
    }
    let o = &m.observability;
    out.push("photon:".to_string());
    out.push(format!("  otlp_endpoint: {}", o.otlp_endpoint));
    out.push(format!("  ingest_token: {}", o.ingest_token));
    out.push(format!(
        "  service_name: {}",
        o.service_name
            .as_deref()
            .map_or("null".to_string(), yaml_str)
    ));
    out.push(format!("  environment: {}", o.environment));
    out.join("\n") + "\n"
}

fn variable_type_str(kind: VariableType) -> &'static str {
    match kind {
        VariableType::String => "string",
        VariableType::Number => "number",
        VariableType::Bool => "bool",
        VariableType::Select => "select",
        VariableType::Port => "port",
        VariableType::Path => "path",
        VariableType::Secret => "secret",
        VariableType::Generated => "generated",
    }
}

// ---------------------------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------------------------

/// A small backtracking regex, enough for a manifest's `pattern` (`^[0-9]{1,3}$`, `^https?://`)
/// without pulling a regex engine into the domain crate: literals, `.`, `^`, `$`, classes with
/// ranges and negation, `\d \w \s` (and their negations, `\b` excluded), groups with `|`, and
/// the quantifiers `* + ? {n} {n,} {n,m}` (a trailing `?` for laziness is accepted and treated
/// as greedy — it cannot change whether a whole match exists). Anything else fails to compile,
/// and a pattern that does not compile does not block a deploy.
pub mod pattern {
    #[derive(Debug, Clone)]
    enum ClassItem {
        Char(char),
        Range(char, char),
        Digit(bool),
        Word(bool),
        Space(bool),
    }

    #[derive(Debug, Clone)]
    enum Node {
        Char(char),
        Any,
        Class {
            negated: bool,
            items: Vec<ClassItem>,
        },
        Start,
        End,
        Group(Vec<Vec<Node>>),
        Repeat {
            node: Box<Node>,
            min: usize,
            max: Option<usize>,
        },
    }

    #[derive(Debug, Clone)]
    pub struct Pattern {
        alts: Vec<Vec<Node>>,
    }

    struct Parser {
        chars: Vec<char>,
        i: usize,
    }

    impl Parser {
        fn peek(&self) -> Option<char> {
            self.chars.get(self.i).copied()
        }
        fn next(&mut self) -> Option<char> {
            let c = self.peek()?;
            self.i += 1;
            Some(c)
        }

        fn alternation(&mut self) -> Result<Vec<Vec<Node>>, String> {
            let mut alts = vec![self.sequence()?];
            while self.peek() == Some('|') {
                self.i += 1;
                alts.push(self.sequence()?);
            }
            Ok(alts)
        }

        fn sequence(&mut self) -> Result<Vec<Node>, String> {
            let mut seq = Vec::new();
            while let Some(c) = self.peek() {
                if c == '|' || c == ')' {
                    break;
                }
                let atom = self.atom()?;
                seq.push(self.quantified(atom)?);
            }
            Ok(seq)
        }

        fn atom(&mut self) -> Result<Node, String> {
            let c = self.next().ok_or("unexpected end")?;
            Ok(match c {
                '(' => {
                    if self.peek() == Some('?') {
                        // Only the non-capturing marker is understood.
                        if self.chars.get(self.i + 1) == Some(&':') {
                            self.i += 2;
                        } else {
                            return Err("lookaround is not supported".to_string());
                        }
                    }
                    let alts = self.alternation()?;
                    if self.next() != Some(')') {
                        return Err("unclosed group".to_string());
                    }
                    Node::Group(alts)
                }
                ')' => return Err("unmatched )".to_string()),
                '[' => self.class()?,
                '\\' => match self.escape()? {
                    ClassItem::Char(ch) => Node::Char(ch),
                    item => Node::Class {
                        negated: false,
                        items: vec![item],
                    },
                },
                '.' => Node::Any,
                '^' => Node::Start,
                '$' => Node::End,
                '*' | '+' | '?' => return Err(format!("nothing to repeat before {c}")),
                '{' => Node::Char('{'),
                other => Node::Char(other),
            })
        }

        fn escape(&mut self) -> Result<ClassItem, String> {
            let c = self.next().ok_or("trailing backslash")?;
            Ok(match c {
                'd' => ClassItem::Digit(true),
                'D' => ClassItem::Digit(false),
                'w' => ClassItem::Word(true),
                'W' => ClassItem::Word(false),
                's' => ClassItem::Space(true),
                'S' => ClassItem::Space(false),
                'n' => ClassItem::Char('\n'),
                't' => ClassItem::Char('\t'),
                'r' => ClassItem::Char('\r'),
                'b' | 'B' => return Err("word boundaries are not supported".to_string()),
                c if c.is_ascii_digit() => {
                    return Err("backreferences are not supported".to_string())
                }
                c if c.is_ascii_alphanumeric() => return Err(format!("unknown escape \\{c}")),
                other => ClassItem::Char(other),
            })
        }

        fn class(&mut self) -> Result<Node, String> {
            let negated = self.peek() == Some('^');
            if negated {
                self.i += 1;
            }
            let mut items = Vec::new();
            let mut first = true;
            loop {
                let c = self.next().ok_or("unclosed character class")?;
                if c == ']' && !first {
                    break;
                }
                first = false;
                let item = if c == '\\' {
                    self.escape()?
                } else {
                    ClassItem::Char(c)
                };
                // A range `a-z` — only between two plain chars.
                if let ClassItem::Char(lo) = item {
                    if self.peek() == Some('-')
                        && self.chars.get(self.i + 1).is_some_and(|c| *c != ']')
                    {
                        self.i += 1;
                        let hi_c = self.next().ok_or("unclosed character class")?;
                        let hi = if hi_c == '\\' {
                            match self.escape()? {
                                ClassItem::Char(h) => h,
                                _ => return Err("bad range in character class".to_string()),
                            }
                        } else {
                            hi_c
                        };
                        if lo > hi {
                            return Err("range out of order in character class".to_string());
                        }
                        items.push(ClassItem::Range(lo, hi));
                        continue;
                    }
                }
                items.push(item);
            }
            Ok(Node::Class { negated, items })
        }

        fn quantified(&mut self, atom: Node) -> Result<Node, String> {
            let (min, max) = match self.peek() {
                Some('*') => (0, None),
                Some('+') => (1, None),
                Some('?') => (0, Some(1)),
                Some('{') => match self.braces()? {
                    Some(r) => r,
                    None => return Ok(atom),
                },
                _ => return Ok(atom),
            };
            // The cursor sits on the marker (`*`, `+`, `?`) or on a quantifier's closing `}`.
            self.i += 1;
            if self.peek() == Some('?') {
                self.i += 1;
            }
            if matches!(atom, Node::Start | Node::End) {
                return Err("nothing to repeat".to_string());
            }
            Ok(Node::Repeat {
                node: Box::new(atom),
                min,
                max,
            })
        }

        /// `{n}`, `{n,}`, `{n,m}` at the cursor (which sits ON the `{`); a `{` that is not a
        /// quantifier is a literal, so `None` leaves the cursor in place.
        fn braces(&mut self) -> Result<Option<(usize, Option<usize>)>, String> {
            let save = self.i;
            self.i += 1;
            let num = |p: &mut Parser| -> Option<usize> {
                let start = p.i;
                while p.peek().is_some_and(|c| c.is_ascii_digit()) {
                    p.i += 1;
                }
                if p.i == start {
                    return None;
                }
                p.chars[start..p.i].iter().collect::<String>().parse().ok()
            };
            let Some(min) = num(self) else {
                self.i = save;
                return Ok(None);
            };
            let max = if self.peek() == Some(',') {
                self.i += 1;
                if self.peek() == Some('}') {
                    None
                } else {
                    match num(self) {
                        Some(m) => Some(m),
                        None => {
                            self.i = save;
                            return Ok(None);
                        }
                    }
                }
            } else {
                Some(min)
            };
            if self.peek() != Some('}') {
                self.i = save;
                return Ok(None);
            }
            // Leave the cursor on the `}` so `quantified` steps past it like any other marker.
            if max.is_some_and(|m| m < min) {
                return Err("quantifier range out of order".to_string());
            }
            Ok(Some((min, max)))
        }
    }

    /// Compiles a JS-style pattern; `Err` when it uses something this matcher does not do.
    pub fn compile(pattern: &str) -> Result<Pattern, String> {
        let mut p = Parser {
            chars: pattern.chars().collect(),
            i: 0,
        };
        let alts = p.alternation()?;
        if p.i != p.chars.len() {
            return Err("unmatched )".to_string());
        }
        Ok(Pattern { alts })
    }

    /// `Some(matched)` when the pattern compiles, `None` when it does not.
    pub fn matches(pattern: &str, text: &str) -> Option<bool> {
        compile(pattern).ok().map(|p| p.is_match(text))
    }

    fn item_matches(item: &ClassItem, c: char) -> bool {
        match item {
            ClassItem::Char(x) => *x == c,
            ClassItem::Range(lo, hi) => (*lo..=*hi).contains(&c),
            ClassItem::Digit(yes) => c.is_ascii_digit() == *yes,
            ClassItem::Word(yes) => (c.is_ascii_alphanumeric() || c == '_') == *yes,
            ClassItem::Space(yes) => c.is_whitespace() == *yes,
        }
    }

    fn match_one(node: &Node, t: &[char], pos: usize) -> Option<usize> {
        match node {
            Node::Start => (pos == 0).then_some(pos),
            Node::End => (pos == t.len()).then_some(pos),
            Node::Any => (pos < t.len()).then_some(pos + 1),
            Node::Char(c) => (t.get(pos) == Some(c)).then_some(pos + 1),
            Node::Class { negated, items } => {
                let c = *t.get(pos)?;
                let hit = items.iter().any(|i| item_matches(i, c));
                (hit != *negated).then_some(pos + 1)
            }
            Node::Group(_) | Node::Repeat { .. } => unreachable!("handled by match_node"),
        }
    }

    fn match_node(node: &Node, t: &[char], pos: usize, k: &dyn Fn(usize) -> bool) -> bool {
        match node {
            Node::Group(alts) => alts.iter().any(|alt| match_seq(alt, t, pos, k)),
            Node::Repeat { node, min, max } => repeat(node, *min, *max, t, pos, 0, k),
            single => match_one(single, t, pos).is_some_and(k),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn repeat(
        node: &Node,
        min: usize,
        max: Option<usize>,
        t: &[char],
        pos: usize,
        count: usize,
        k: &dyn Fn(usize) -> bool,
    ) -> bool {
        // Greedy: one more repetition first (an empty iteration is not progress), then stop.
        if max.is_none_or(|m| count < m)
            && match_node(node, t, pos, &|p| {
                p != pos && repeat(node, min, max, t, p, count + 1, k)
            })
        {
            return true;
        }
        count >= min && k(pos)
    }

    fn match_seq(seq: &[Node], t: &[char], pos: usize, k: &dyn Fn(usize) -> bool) -> bool {
        match seq.split_first() {
            None => k(pos),
            Some((first, rest)) => match_node(first, t, pos, &|p| match_seq(rest, t, p, k)),
        }
    }

    impl Pattern {
        /// Unanchored search, like JS `RegExp.test`.
        pub fn is_match(&self, text: &str) -> bool {
            let t: Vec<char> = text.chars().collect();
            (0..=t.len()).any(|start| {
                self.alts
                    .iter()
                    .any(|alt| match_seq(alt, &t, start, &|_| true))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TemplateObservability, TemplateRequires};

    fn var(name: &str, kind: VariableType, required: bool) -> TemplateVariable {
        TemplateVariable {
            name: name.to_string(),
            label: name.to_string(),
            description: None,
            kind,
            required,
            default: None,
            preset: None,
            options: None,
            pattern: None,
            group: None,
        }
    }

    fn manifest() -> TemplateManifest {
        let mut version = var("VERSION", VariableType::String, true);
        version.default = Some("1.0.0".into());
        version.preset = Some(true);
        let mut port = var("PORT", VariableType::Port, true);
        port.default = Some("8443".into());
        port.group = Some("Network".into());
        let mut model = var("MODEL", VariableType::Select, true);
        model.options = Some(vec!["a".into(), "b".into()]);
        let debug = var("DEBUG", VariableType::Bool, false);
        let mut data = var("DATA", VariableType::Path, false);
        data.default = Some("/data".into());
        TemplateManifest {
            id: "t".into(),
            version: "1.0.0".into(),
            name: "T".into(),
            description: String::new(),
            icon: String::new(),
            categories: vec![],
            variables: vec![
                version,
                port,
                model,
                debug,
                data,
                var("LICENSE", VariableType::Secret, true),
                var("DB_PASSWORD", VariableType::Generated, true),
            ],
            requires: TemplateRequires {
                arch: Some(vec!["x86_64".into()]),
                min_docker: Some("24.0".into()),
                min_ram_bytes: None,
                min_disk_bytes: None,
            },
            ports: vec![],
            volumes: vec![],
            health: None,
            observability: TemplateObservability {
                otlp_endpoint: true,
                ingest_token: true,
                service_name: None,
                environment: true,
            },
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn seeds_defaults_skipping_secrets_and_generated_values() {
        let m = manifest();
        assert_eq!(
            default_variables(&m, &BTreeMap::new()),
            map(&[
                ("VERSION", "1.0.0"),
                ("PORT", "8443"),
                ("MODEL", "a"),
                ("DEBUG", "false"),
                ("DATA", "/data")
            ])
        );
        assert_eq!(
            default_variables(&m, &map(&[("PORT", "9000")]))["PORT"],
            "9000"
        );
    }

    #[test]
    fn a_preset_takes_the_template_default_even_when_the_stack_stored_an_older_pin() {
        let d = default_variables(&manifest(), &map(&[("VERSION", "0.9.0"), ("PORT", "9000")]));
        assert_eq!(d["VERSION"], "1.0.0");
        assert_eq!(d["PORT"], "9000");
    }

    #[test]
    fn validates_by_type_and_requires_a_secret_unless_the_store_has_it() {
        let m = manifest();
        let errors = validate_variables(
            &m,
            &map(&[
                ("VERSION", "1"),
                ("PORT", "99999"),
                ("MODEL", "zzz"),
                ("DEBUG", "maybe"),
                ("DATA", "relative"),
            ]),
            &BTreeSet::new(),
        );
        assert_eq!(errors["PORT"], "Must be a port between 1 and 65535.");
        assert_eq!(errors["MODEL"], "Must be one of a, b.");
        assert_eq!(errors["DEBUG"], "Must be true or false.");
        assert_eq!(errors["DATA"], "Must be an absolute path on the host.");
        assert_eq!(
            errors["LICENSE"],
            "Required — enter it once; it is stored encrypted."
        );
        assert!(!errors.contains_key("DB_PASSWORD"));
        assert!(!errors.contains_key("VERSION"));
        let provided: BTreeSet<String> = ["LICENSE".to_string()].into_iter().collect();
        assert!(
            validate_variables(&m, &default_variables(&m, &BTreeMap::new()), &provided).is_empty()
        );
        // A missing required select is "Required."; a missing optional one passes.
        let mut m2 = manifest();
        m2.variables
            .retain(|v| v.name == "MODEL" || v.name == "DEBUG");
        let e = validate_variables(&m2, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(e.get("MODEL").map(String::as_str), Some("Required."));
        assert!(!e.contains_key("DEBUG"));
    }

    #[test]
    fn number_and_pattern_checks() {
        let mut n = var("N", VariableType::Number, false);
        assert_eq!(validate_variable(&n, Some("1.5")), None);
        assert_eq!(validate_variable(&n, Some("-3")), None);
        assert_eq!(
            validate_variable(&n, Some("1.")),
            Some("Must be a number.".into())
        );
        assert_eq!(
            validate_variable(&n, Some("abc")),
            Some("Must be a number.".into())
        );
        n.pattern = Some("^[0-9]{1,3}$".into());
        assert_eq!(validate_variable(&n, Some("16")), None);
        assert_eq!(
            validate_variable(&n, Some("1600")),
            Some("Must match ^[0-9]{1,3}$.".into())
        );
        let mut u = var("PLAYLIST_URL", VariableType::String, true);
        u.pattern = Some("^https?://".into());
        assert_eq!(validate_variable(&u, Some("https://x")), None);
        assert_eq!(
            validate_variable(&u, Some("ftp://x")),
            Some("Must match ^https?://.".into())
        );
        // A pattern the matcher cannot evaluate must not block the deploy.
        u.pattern = Some("(?=x)".into());
        assert_eq!(validate_variable(&u, Some("anything")), None);
        // A preset's default stands in for an empty value.
        let mut p = var("P", VariableType::Port, true);
        p.preset = Some(true);
        p.default = Some("80".into());
        assert_eq!(validate_variable(&p, None), None);
        assert_eq!(validate_variable(&p, Some("  ")), None);
    }

    #[test]
    fn pattern_matcher_covers_the_common_shapes() {
        use pattern::matches;
        assert_eq!(matches("^[0-9]{1,3}$", "123"), Some(true));
        assert_eq!(matches("^[0-9]{1,3}$", "1234"), Some(false));
        assert_eq!(matches("^[0-9]{1,3}$", ""), Some(false));
        assert_eq!(matches("^https?://", "http://a"), Some(true));
        assert_eq!(matches("^https?://", "xhttp://a"), Some(false));
        assert_eq!(matches("a.c", "xxabcxx"), Some(true));
        assert_eq!(matches("^(foo|bar)+$", "foobarfoo"), Some(true));
        assert_eq!(matches("^(foo|bar)+$", "foobaz"), Some(false));
        assert_eq!(matches("^\\d+(\\.\\d+)?$", "3.14"), Some(true));
        assert_eq!(matches("^\\d+(\\.\\d+)?$", "3."), Some(false));
        assert_eq!(matches("^[^/]+$", "abc"), Some(true));
        assert_eq!(matches("^[^/]+$", "a/b"), Some(false));
        assert_eq!(matches("^[a-z][a-z0-9_-]*$", "my-stack_1"), Some(true));
        assert_eq!(matches("^[a-z][a-z0-9_-]*$", "My"), Some(false));
        assert_eq!(matches("^\\w+@\\w+\\.\\w{2,}$", "a@b.io"), Some(true));
        assert_eq!(matches("x{2}", "axxb"), Some(true));
        assert_eq!(matches("x{3,}", "axxb"), Some(false));
        assert_eq!(matches("^a{b$", "a{b"), Some(true)); // a `{` that is not a quantifier is literal
        assert_eq!(matches("^(?:ab)*c$", "ababc"), Some(true));
        assert_eq!(matches("^\\s*$", "   "), Some(true));
        assert_eq!(matches("colou?r", "color"), Some(true));
        assert_eq!(matches("^a*?b$", "aab"), Some(true));
        assert_eq!(matches("(?=x)", "x"), None);
        assert_eq!(matches("\\bx", "x"), None);
        assert_eq!(matches("(a", "a"), None);
        assert_eq!(matches("a)", "a"), None);
        assert_eq!(matches("[a-", "a"), None);
        assert_eq!(matches("*a", "a"), None);
        assert_eq!(matches("(a*)*b", "aaaaaaaaaaaaaaaaaaaaaaaac"), Some(false));
        // terminates
    }

    #[test]
    fn renders_env_without_secret_values_and_with_the_observability_block() {
        let m = manifest();
        let values = default_variables(&m, &BTreeMap::new());
        let r = render(RenderInput {
            manifest: &m,
            compose_text: "services:\n  app:\n    image: x:${VERSION}\n    ports:\n      - \"${PORT}:80\"\n    environment:\n      LICENSE: ${LICENSE}\n      OTEL: ${OTEL_EXPORTER_OTLP_ENDPOINT}\n",
            values: &values,
            otlp_endpoint: Some("http://p:4318"),
            ingest_token: Some("tok"),
            environment: Some("prod"),
        });
        assert!(r.env.contains("PORT=8443\n"));
        assert!(r
            .env
            .contains("OTEL_EXPORTER_OTLP_ENDPOINT=http://p:4318\n"));
        assert!(r
            .env
            .contains("OTEL_EXPORTER_OTLP_HEADERS=Authorization=Bearer tok\n"));
        assert!(r.env.contains("DEPLOYMENT_ENVIRONMENT=prod\n"));
        assert!(r
            .env
            .contains("OTEL_RESOURCE_ATTRIBUTES=deployment.environment=prod\n"));
        assert!(!r.env.contains("OTEL_SERVICE_NAME"));
        // Secrets are bare placeholders for the agent: a name, never a value.
        assert!(r.env.contains("\nLICENSE=\n"));
        assert!(r.env.contains("\nDB_PASSWORD=\n"));
        assert!(r.env.ends_with('\n'));
        assert_eq!(
            r.env,
            "VERSION=1.0.0\nPORT=8443\nMODEL=a\nDEBUG=false\nDATA=/data\nLICENSE=\nDB_PASSWORD=\nOTEL_EXPORTER_OTLP_ENDPOINT=http://p:4318\nPHOTON_OTLP_HTTP_ENDPOINT=http://p:4318\nOTEL_EXPORTER_OTLP_HEADERS=Authorization=Bearer tok\nPHOTON_OTLP_TOKEN=tok\nDEPLOYMENT_ENVIRONMENT=prod\nOTEL_RESOURCE_ATTRIBUTES=deployment.environment=prod\n"
        );
        // The compose gets the non-secret values and the endpoint; the secret ref stays a ref.
        assert!(r.compose.contains("image: x:1.0.0"));
        assert!(r.compose.contains("\"8443:80\""));
        assert!(r.compose.contains("LICENSE: ${LICENSE}"));
        assert!(r.compose.contains("OTEL: http://p:4318"));
        assert_eq!(r.secret_names, vec!["LICENSE"]);
        assert_eq!(r.generated_names, vec!["DB_PASSWORD"]);
        assert_eq!(r.apm_services, vec!["app"]);
    }

    #[test]
    fn render_never_substitutes_a_secret_value_handed_in_by_mistake() {
        let m = manifest();
        let mut values = default_variables(&m, &BTreeMap::new());
        values.insert("LICENSE".into(), "hunter2".into());
        let r = render(RenderInput {
            manifest: &m,
            compose_text: "x: ${LICENSE}\n",
            values: &values,
            otlp_endpoint: None,
            ingest_token: None,
            environment: None,
        });
        assert_eq!(r.compose, "x: ${LICENSE}\n");
        assert!(!r.env.contains("hunter2"));
        assert!(!r.env.contains("OTEL_"));
    }

    #[test]
    fn env_block_is_gated_on_the_photon_block() {
        let mut m = manifest();
        m.observability = TemplateObservability {
            otlp_endpoint: false,
            ingest_token: false,
            service_name: Some("kiosk".into()),
            environment: false,
        };
        let r = render(RenderInput {
            manifest: &m,
            compose_text: "",
            values: &BTreeMap::new(),
            otlp_endpoint: Some("http://p:4318"),
            ingest_token: Some("tok"),
            environment: Some("prod"),
        });
        assert!(!r.env.contains("OTEL_EXPORTER_OTLP_ENDPOINT"));
        assert!(!r.env.contains("Bearer"));
        assert!(!r.env.contains("DEPLOYMENT_ENVIRONMENT"));
        assert!(r.env.contains("OTEL_SERVICE_NAME=kiosk\n"));
        assert!(r.apm_services.is_empty());
    }

    #[test]
    fn compares_and_bumps_versions() {
        assert_eq!(semver_compare("1.10.0", "1.9.2"), Ordering::Greater);
        assert_eq!(semver_compare("2.4.1", "2.4.1"), Ordering::Equal);
        assert_eq!(semver_compare("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(semver_compare("1.2.0-rc1", "1.2.0-rc2"), Ordering::Less);
        assert_eq!(semver_compare("0.9", "1.0"), Ordering::Less);
        assert_eq!(next_version("2.4.1"), "2.4.2");
        assert_eq!(next_version("1.2.0-rc1"), "1.2.0-rc2");
        assert_eq!(next_version("v007"), "v8");
        assert_eq!(next_version("abc"), "abc.1");
    }

    #[test]
    fn renders_a_manifest_as_yaml_with_the_photon_block() {
        let y = manifest_to_yaml(&manifest());
        assert!(y.starts_with("id: t\nversion: \"1.0.0\"\nname: T\ndescription: \"\"\nicon: \"\"\ncategories: []\nvariables:\n"));
        assert!(y.contains("  - name: PORT\n    type: port\n    label: PORT\n    required: true\n    default: \"8443\"\n    group: Network\n"));
        assert!(y.contains("  - name: VERSION\n    type: string\n    label: VERSION\n    required: true\n    default: \"1.0.0\"\n    preset: true\n"));
        assert!(y.contains("options: [a, b]"));
        assert!(y.contains("requires:\n  arch: [x86_64]\n  min_docker: \"24.0\"\n"));
        assert!(y.ends_with("photon:\n  otlp_endpoint: true\n  ingest_token: true\n  service_name: null\n  environment: true\n"));
        assert_eq!(yaml_str("plain/path:x"), "plain/path:x");
        assert_eq!(yaml_str("two words"), "\"two words\"");
        assert_eq!(yaml_str("yes"), "\"yes\"");
        assert_eq!(yaml_str("12ab"), "\"12ab\"");
        assert_eq!(yaml_str("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn manifest_validation() {
        let mut m = manifest();
        assert_eq!(validate_manifest(&m), Ok(()));
        m.id = "Bad Id".into();
        assert!(validate_manifest(&m).unwrap_err().contains("id"));
        m.id = "ok-id".into();
        m.version = "one".into();
        assert!(validate_manifest(&m).unwrap_err().contains("version"));
        m.version = "1.2.0-rc1".into();
        m.name = " ".into();
        assert!(validate_manifest(&m).unwrap_err().contains("name"));
        m.name = "n".into();
        m.variables[1].name = "port".into();
        assert!(validate_manifest(&m).unwrap_err().contains("env-style"));
        m.variables[1].name = "VERSION".into();
        assert!(validate_manifest(&m).unwrap_err().contains("twice"));
        m.variables[1].name = "PORT".into();
        m.variables[2].options = Some(vec![]);
        assert!(validate_manifest(&m).unwrap_err().contains("no options"));
        m.variables[2].options = Some(vec!["a".into()]);
        m.variables[0].pattern = Some("(?=x)".into());
        assert!(validate_manifest(&m).unwrap_err().contains("pattern"));
        m.variables[0].pattern = Some("^[0-9]+$".into());
        m.ports.push(crate::model::TemplatePort {
            name: "web".into(),
            container: 0,
            default_host: 80,
            protocol: "tcp".into(),
            description: None,
        });
        assert!(validate_manifest(&m).unwrap_err().contains("65535"));
        m.ports[0].container = 80;
        m.ports[0].protocol = "sctp".into();
        assert!(validate_manifest(&m).unwrap_err().contains("protocol"));
        m.ports[0].protocol = "udp".into();
        assert_eq!(validate_manifest(&m), Ok(()));
        assert!(is_version("1"));
        assert!(is_version("1.2.3+build.5"));
        assert!(!is_version("1..2"));
        assert!(!is_version("1.2-"));
        assert!(!is_version(""));
    }

    #[test]
    fn generated_secrets_are_hex_and_unique() {
        let a = generate_secret();
        let b = generate_secret();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
