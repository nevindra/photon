# Domain model

The project's ubiquitous language. Glossary only — no implementation detail, no specs.

## Signals & worlds

**Signal** — one kind of telemetry Photon carries: logs, traces, metrics, uptime, RUM. Each is
independent end to end.

**World** — how the UI groups signals by who owns them: Frontend (RUM), Backend (services/APM),
Infrastructure (hosts + uptime). A world is an ownership boundary, not a signal.

**Subject** — a named thing that has an owner, can be judged healthy or not, and has a page of its
own. Five kinds: a Service, a RUM app, a Host, a Monitor, a [[tenant]]. A Subject belongs to one
World. It is the unit that answers "where is it wrong?", so anything that cannot answer "where" is
not a Subject. Deliberately NOT called Entity (taken: the "Related ▾" navigation source), Component
(taken: Vue components), or Target (the alerts vocabulary for what a rule is *about* — that set
lacks Monitor and includes `global`, which is the absence of a place).

A Subject is identified by **(Site, kind, name)**, never by name alone. `payments-api` in `eu-west`
and `payments-api` in `us-east` are two Subjects: same software, two deployments, two things that
can fail independently. [[central]]'s own local Subjects carry no Site, and that absence is part of
the identity rather than a missing value.

**Spread** — on [[central]], how many Sites a Service or RUM app is unhealthy in, out of how many
run it ("3 of 3", "1 of 3"). Spread is a *diagnosis*, not a count: unhealthy everywhere points at
the software, unhealthy in one Site points at that Site. Only Services and RUM apps have a Spread,
because only they are one piece of software deployed many times. Hosts and Monitors never do —
`web-03` in `eu-west` and `web-03` in `us-east` are two machines that happen to share a hostname,
and merging them would invent a thing that does not exist.

**Site** — one Photon deployment of *our own*, in another region / cluster / environment, registered
by a Central that aggregates the fleet. A Site is ours, not a customer's: when a Site goes quiet
that is our outage, which is why it is a [[subject]] and sits in the same attention list as a
Service or a Host. The Site chooses which signals it ships; Central sees only what arrives, so a
Site that ships traces only contributes Services to Central and no Hosts.

*Code and config call this a **tenant*** (`tenants` table, `?tenant=`, the `tenant` resource
attribute, `[federation]`), and that name is not going to change. We say **Site** when we think,
because "tenant" reads as *customer* — which is exactly the drift that once had
`docs/subsystems/federation.md` describing Sites as "customer Photon installs", and `CLAUDE.md` /
`README.md` listing clustering as out of scope. All three now match this entry: federating a fleet
of our own installs is not clustering, and it is not something we sell to a customer.

**Central** — the Photon a fleet of [[site]]s pushes to. Central is not a different product: it runs
the same code and the same landing page. The difference is only in the data — on Central every
Subject carries a Site, on a Site install none do. Readers split the same way: the central team
reads Central, a site team reads their own Site's Photon. Nobody is *restricted* to a Site by
Photon; there are no roles yet, so scoping is what the data already is, never an enforced boundary.

**Attention Level** — the one severity axis the landing page ranks [[subject]]s on, so that a Host
and a Service can be compared at all. Four levels — **Down** (not answering), **Critical** (answering
badly, user-visible), **Warning** (degraded, not yet breaking), **OK** — plus **Unknown**, which is
not a level but the *absence* of a judgment (no traffic, paused, never reported). Each kind's native
word (`degraded`, `poor`, `down`, `error`) survives verbatim in the Subject's reason line; only the
ranking is unified. A slow Web Vital is Warning, never Critical — a page that cries wolf about
latency stops being read.

**Silence** — a Subject that reported inside the [[watch-range]] and then stopped for several
expected intervals. Silence is **Down**, because the stopping is itself the news. A Subject that
never reported in the range is **Unknown**, not Down: Photon cannot tell a dead machine from an
uninstalled agent, so it says which one it observed rather than guessing.

**Unattributed Incident** — a triggered alert incident that resolves to no [[subject]], because its
rule is genuinely about the fleet rather than a place (a metrics rule with no `group_by`) or because
the place is not recoverable from the rule. It is still shown — never hidden — just not as a Subject.
An incident may also resolve to *several* Subjects (a logs rule naming two services); that is
allowed, and the incident appears on each.

## Backend (services / APM)

**Service** — one named producer of spans, identified by `service.name`. The unit of ownership on
the Backend world.

**Operation** — one named span within a service (`name`). A service has many operations.

**Service Health** — a Service's classification into `critical` / `degraded` / `healthy` / `idle`
over the active time window, decided by error rate OR Apdex, whichever is worse. No traffic means
`idle`, never a colour. Health is a *derived, window-scoped* judgment, not stored state.

**Apdex** — a Service's satisfaction score over the window, from the share of spans that are
satisfied (≤ T), tolerating (T..4T), or frustrated (> 4T). T is per-Service, overridable.

**Backend Issue** — a group of span errors sharing the same `(service, backend error fingerprint)`.
Identity is the pair, not the fingerprint alone: the same exception in two Services is two Issues,
because they have different owners and different fixes. Fingerprint is derived from the span's
exception event (type + digit-masked message + top frame), falling back to `status_message` when
the span carries no exception event. Compare [[rum-issue]] — the same idea on the Frontend world,
already implemented.

**RUM Issue** — a group of browser errors sharing a `rum.error.fingerprint`, scoped to a RUM app.
The Frontend-world counterpart of a Backend Issue. Deliberately a separate concept: it is grouped
per app, not per Service, and its evidence is sessions rather than spans.

**Dependency** — a downstream call a Service makes, split by kind: a Database dependency (a CLIENT
span carrying `db.system`) or an External dependency (a CLIENT span naming a peer). A Dependency is
*not* a Service, even when the peer happens to be one.

## Time

**Window** — the app-wide active time range. Every number on every Backend page is scoped to it.

**Previous Window** — the equally-long range immediately before the Window. The only comparison
basis for a trend chip; there is no baseline or seasonal model.

**Watch Range** — the short, deliberately bounded lookback the landing page judges over (minutes to
a few hours, never days). It is not the [[window]] and does not read it: a judgment of "wrong now"
averaged over a week is a lie, so the range that produces it is capped by design. Drilling out of
the landing page carries the Watch Range, so the page you land on shows the same slice of time.
