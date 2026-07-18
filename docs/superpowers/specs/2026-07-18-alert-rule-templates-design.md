# Alert Rule Templates (Quick Setup) — Design

**Status:** Approved (grilled 2026-07-18)
**Feature:** A curated, read-only catalog of ready-made alert rules so a user can stand up
standard alerting for a service / app / host in one or two clicks, instead of hand-building each
condition. Builds directly on the webhook alert engine shipped in `feat/webhook-alerts`.

## 1. Goal

Let users go from "I have a new service `checkout-api`" to "it has sensible error-rate, latency and
log alerts" in seconds. A **target-first** flow: pick what you're monitoring → see the templates
that apply → **Apply** one directly, or **Customize** it first.

## 2. Nature & scope

**Frontend-only.** No backend, no new storage, no new API, no migration. The catalog is static,
read-only seed data baked into the UI. Every applied template becomes a normal rule through the
**existing** `POST /api/alerts/rules`, and "Customize" opens the **existing** `AlertRuleDialog`.

Verified load-bearing fact: `photon-server/src/alerts_source.rs::sample_metrics` compiles a
metric condition's `label_filters` into the query filter (`build_metric_filter` →
`MetricSeriesRequest.labels`), so **Host** templates (which scope by `host.name`) work against the
current backend with zero changes.

### Non-goals (explicit)

- **Provider-native channels** (Slack/Discord/Teams/PagerDuty message formatting) — booked as the
  *next* feature. Channels remain the single generic `Webhook` kind.
- **User-saved / user-authored templates** — the catalog is ours, in code.
- **A backend catalog or `/api/alerts/templates` endpoint** — the catalog is inert seed data the UI
  holds; an HTTP round-trip buys nothing today. (Promoting it to Rust later is mechanical.)
- **Batch multi-select** — one template at a time.

## 3. User flow

1. **Entry point.** A **"Browse templates"** button next to "New alert" on the Rules tab; the Rules
   **empty state** also links to it ("No rules yet — start from a template").
2. **Template-picker modal** opens:
   - **Top: unified target picker** — segmented `Service · App · Host · Global`.
     - Service / App / Host also show a **target selector** populated from live data
       (`useServices`, `useRumApps`, the infra hosts query). Global needs no selection.
   - **Body: template list** for the selected target *type* — each row shows the template's default
     condition in plain English + a severity pill, and two actions: **Apply** · **Customize**.
   - **One shared optional "Notify" channel multiselect** (feeds Apply). Default none; zero allowed.
     When no channels exist, show the hint: *"No channels yet; the rule will be created without
     notifications — add one on the Channels tab."*
3. **Apply directly** → build a `RuleInput` from the template with the target substituted in →
   `POST /api/alerts/rules` → success toast → close modal + refetch rules.
4. **Customize** → close the picker, open `AlertRuleDialog` **pre-seeded** from the template draft
   (create mode, sequential modals — never stacked) → user edits anything → normal save path.

A target selector is **required** before Apply/Customize for Service/App/Host templates (the row
actions are disabled until a target is chosen); Global templates are always actionable.

## 4. Data model (frontend)

A template is a typed constant. The catalog lives in **`frontend/src/lib/alertTemplates.ts`**,
typed against the existing `AlertCondition` union in `lib/core/api.ts`.

```ts
export type TemplateTarget = 'service' | 'app' | 'host' | 'global'

export interface AlertTemplate {
  id: string                 // stable slug, e.g. 'svc-high-error-rate'
  target: TemplateTarget
  name: string               // e.g. 'High error rate'
  description: string        // one line shown in the row
  severity: AlertSeverity    // 'info' | 'warning' | 'critical'
  for_secs: number
  interval_secs?: number     // default 60
  // Condition with the target-specific field left as a placeholder the substitution fills.
  // `build(target)` returns a concrete AlertCondition (see §5).
  build: (target: string) => AlertCondition
}
```

`build(target)` is what performs **target substitution**:

| Target  | Substitution into the `AlertCondition` |
|---------|----------------------------------------|
| Service | traces → `service = target`; logs → prepend `service.name:<target>` to `query` |
| App     | rum → `app_id = target` |
| Host    | metrics → `label_filters = { 'host.name': target }` |
| Global  | metrics → `group_by = ['host.name']`, no host filter (one series per host) |

### 4.1 Required small type/UI extensions

- **`MetricsCondition` gains `label_filters?: Record<string, string>`** in `lib/core/api.ts`
  (the backend `MetricCondition` already has `label_filters`, `#[serde(default)]`). Host templates
  emit it; the mock/api types must carry it so it survives a `POST`.
- **`ConditionBuilder` must round-trip `label_filters`.** Today its metrics `seed()`/`builtCondition`
  drop everything except `metric_name`/`agg`/`group_by`. So a Customized **Host** template would
  silently lose its `host.name` scope. Fix: seed `form.label_filters` from the condition, emit it in
  `builtCondition`, and render existing filters as **removable key=value chips** (mirroring the
  existing group-by chip UI). This makes the metrics builder generally more capable and keeps
  Customize honest. (Build the primitive properly — no patching around it.)

## 5. The catalog (23 templates)

Constraints honored: trace latency uses **p50/p90/p99** (engine rejects p95); metric aggs exclude
p95; `system.*.utilization` metrics are **0–1 fractions** (0.9 = 90%).

### Service (traces + logs) — 7
| id | name | condition | for | severity |
|----|------|-----------|-----|----------|
| svc-high-error-rate | High error rate | traces `error_rate > 5` (%) / 5m | 5m | critical |
| svc-elevated-error-rate | Elevated error rate | traces `error_rate > 1` (%) / 5m | 10m | warning |
| svc-slow-p99 | Slow responses (p99) | traces `latency_p99 > 1000` (ms) / 5m | 5m | warning |
| svc-slow-p90 | Slow responses (p90) | traces `latency_p90 > 500` (ms) / 5m | 10m | warning |
| svc-traffic-dropped | Traffic dropped | traces `request_rate < 1` (req/s) / 10m | 10m | warning |
| svc-error-logs | Error logs surging | logs `count(severity:error <svc>) > 100` / 10m | 5m | warning |
| svc-fatal-logs | Fatal logs appeared | logs `count(severity:fatal <svc>) > 0` / 5m | 0 | critical |

### RUM app — 6
| id | name | condition | for | severity |
|----|------|-----------|-----|----------|
| rum-poor-lcp | Poor LCP | rum `vital_lcp_p75 > 2500` (ms) / 15m | 0 | warning |
| rum-poor-inp | Poor INP | rum `vital_inp_p75 > 200` (ms) / 15m | 0 | warning |
| rum-poor-cls | Layout shift (CLS) | rum `vital_cls_p75 > 0.1` / 15m | 0 | warning |
| rum-slow-fcp | Slow FCP | rum `vital_fcp_p75 > 1800` (ms) / 15m | 0 | warning |
| rum-slow-ttfb | Slow TTFB | rum `vital_ttfb_p75 > 800` (ms) / 15m | 0 | warning |
| rum-js-errors | JS errors surging | rum `error_count > 50` / 15m | 0 | warning |

### Host (infra metrics filtered to one host) — 6
| id | name | condition | for | severity |
|----|------|-----------|-----|----------|
| host-cpu | CPU saturated | `avg(system.cpu.utilization) > 0.9` / 5m | 5m | warning |
| host-mem | Memory pressure | `avg(system.memory.utilization) > 0.9` / 5m | 5m | warning |
| host-disk | Disk filling up | `avg(system.filesystem.utilization) > 0.85` / 10m | 10m | warning |
| host-gpu | GPU saturated | `avg(system.gpu.utilization) > 0.95` / 5m | 5m | warning |
| host-gpu-temp | GPU overheating | `max(system.gpu.temperature) > 85` / 5m | 5m | critical |
| host-gpu-mem | GPU memory pressure | `avg(system.gpu.memory.utilization) > 0.9` / 5m | 5m | warning |

### Global / fleet (metrics grouped by host.name) — 4
| id | name | condition | for | severity |
|----|------|-----------|-----|----------|
| fleet-cpu | Any host CPU saturated | `avg(system.cpu.utilization) by host.name > 0.9` / 5m | 5m | warning |
| fleet-mem | Any host memory pressure | `avg(system.memory.utilization) by host.name > 0.9` / 5m | 5m | warning |
| fleet-disk | Any host disk filling | `avg(system.filesystem.utilization) by host.name > 0.85` / 10m | 10m | warning |
| fleet-gpu-temp | Any GPU overheating | `max(system.gpu.temperature) by host.name > 85` / 5m | 5m | critical |

## 6. Applied-rule defaults

When Apply (or Customize→save) creates the rule:
- **name:** `"<Template name> · <target>"` (Global: just `"<Template name>"`), editable in Customize.
- **enabled:** true. **interval_secs:** 60. **severity / for_secs / condition:** from the template.
- **channel_ids:** whatever the modal's "Notify" multiselect had selected (may be empty).

## 7. UI components (new)

- `frontend/src/lib/alertTemplates.ts` — the typed catalog + `build(target)` substitution.
- `frontend/src/components/alerts/TemplatePickerDialog.vue` — the modal: target picker (segmented) +
  target selector + template list + shared channel multiselect + Apply/Customize per row.
- `frontend/src/components/alerts/TemplateRow.vue` — one template row (plain-English condition,
  severity pill, Apply/Customize).
- Wiring in `AlertsView.vue`: a "Browse templates" button + empty-state link; open the picker; on
  **Customize**, open `AlertRuleDialog` in **create-pre-seeded** mode.
- `AlertRuleDialog.vue`: accept an optional **seed draft** (name + `AlertCondition` + severity +
  `for_secs` + `channel_ids`) that opens it in *create* mode pre-filled — distinct from `:rule`
  (edit). Simplest shape: a `:seed` prop (a partial `RuleInput`) honored only when `:rule` is null.

## 8. Testing

- **Catalog unit tests** (`alertTemplates.test.ts`): every template's `build()` produces a
  structurally valid `AlertCondition`; target substitution lands in the right field for each target
  type; no template uses p95 / `latency_p95`; utilization thresholds are within (0, 1].
- **ConditionBuilder round-trip test:** seeding a metrics condition that has `label_filters`
  re-emits the same `label_filters` (guards the Customize-drops-host-scope regression).
- **Component/flow test (if feasible with the existing harness):** picking a Service target + a
  template and clicking **Apply** issues one `POST /api/alerts/rules` with the substituted condition,
  the composed name, and the selected channels.
- Gate: `bun run type-check` + `bun run test` green; no backend tests touched.

## 9. Docs to update (same change)

- `docs/subsystems/alerts.md` — a "Templates / quick setup" subsection (the flow, that it's
  frontend-only seed data, the catalog location, the non-goal of provider-native channels).
- `docs/frontend.md` — the new components + the `/alerts` template entry point.
- `CLAUDE.md` — one line in the alerts description noting the template quick-setup on-ramp.
- Re-verify internal links.

## 10. Open follow-ups (not this change)

- Provider-native channels (Slack/Discord/Teams/PagerDuty formatting) — the natural next feature.
- Possibly: promote the catalog to a backend endpoint if a headless client ever needs it.
