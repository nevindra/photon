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

A Subject is identified by **(Site, [[environment]], kind, name)**, never by name alone. `payments-api`
in `eu-west` and `payments-api` in `us-east` are two Subjects, and so are `payments-api` in `prod` and
in `staging` inside one Site: same software, different deployments, things that can fail
independently. Merging any of them would average one deployment's failure into another's health.
[[central]]'s own local Subjects carry no Site, and that absence is part of the identity rather than a
missing value.

**Spread** — on [[central]], how many Sites a Service or RUM app is unhealthy in, out of how many
run it ("3 of 3", "1 of 3"). Spread is a *diagnosis*, not a count: unhealthy everywhere points at
the software, unhealthy in one Site points at that Site. It is always read inside one
[[environment]]: comparing one Site's `prod` against another's `staging` diagnoses nothing. Only
Services and RUM apps have a Spread,
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

**Environment** — which deployment of a piece of software a record belongs to: `prod`, `staging`, `dev`.
Part of a [[subject]]'s identity for the same reason [[site]] is, and orthogonal to it: a Site is *where*
software runs, an Environment is *which copy* of it. Photon reads the Environment a producer declares and
never infers one; records that declare none belong to no Environment, which is a fact about the
instrumentation rather than a default worth inventing. Prod is the Environment worth waking someone for,
so it is what the UI selects when it is present, but the others are one click away rather than hidden.

**Central** — the Photon a fleet of [[site]]s pushes to. Central is not a different product: it runs
the same code and the same landing page. The difference is only in the data — on Central every
Subject carries a Site, on a Site install none do. Readers split the same way: the central team
reads Central, a site team reads their own Site's Photon. Nobody is *restricted* to a Site by
Photon; there are no roles yet, so scoping is what the data already is, never an enforced boundary.

**Attention Level** — the one severity axis every page ranks [[subject]]s on, so that a Host
and a Service can be compared at all. Four levels — **Down** (not answering), **Critical** (answering
badly, user-visible), **Warning** (degraded, not yet breaking), **OK** — plus **Unknown**, which is
not a level but the *absence* of a judgment (no traffic, paused, never reported). Each kind's native
word (`degraded`, `poor`, `down`, `error`) survives verbatim in the Subject's reason line; only the
ranking is unified. A slow Web Vital is Warning, never Critical — a page that cries wolf about
latency stops being read. There is no second severity vocabulary anywhere: the Backend world once had
its own (`critical` / `degraded` / `healthy` / `idle`), which let one Service be called two different
things on two pages in the same second. `idle` in particular was one word covering two unrelated
facts, and is retired in favour of [[silence]] and Unknown.

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

**Request** — one unit of work a Service was asked to do, counted from its **entry spans**: the spans
where work crossed into the Service from outside. Everything else a Service emits is its own internal
work, real but not a request. The distinction is load-bearing because a Service that instruments
generously would otherwise look several times busier than one doing identical work, and the two sit in
the same sorted table. A Service that emits no entry spans at all is still counted rather than
disappearing, and the page says which rule produced the number.

Every judgment about a Service — its [[attention-level]], its error rate, its [[apdex]] — is made over
Requests, so a downstream call that failed and was retried successfully never makes the Service look
broken. Health is *derived and window-scoped*, never stored state.

**Apdex** — a Service's satisfaction score over the window, from the share of [[request]]s that are
satisfied (≤ T), tolerating (T..4T), or frustrated (> 4T). T is per-Service, overridable.

**Objective** — a named promise about a Service, either availability (a share of [[request]]s that
must succeed) or latency (a share that must land under a threshold), measured over a rolling window.
A Service may have several, and one may be narrowed to a single [[operation]], because a checkout
endpoint and an admin page in the same Service do not deserve the same promise. Apdex is a *reading*;
an Objective is a *commitment*, and only the second one can be missed.

**Error Budget** — how much of an [[objective]]'s allowed failure is still unspent. **Burn Rate** is
how fast it is going, expressed against the rate that would exactly exhaust it over the window. Budget
alone conveys no urgency: "40% left" reads the same whether it took a month or ten minutes. Burn rate
is the number that tells someone whether to get up.

**Deployment** — a moment when a Service began running a new version. Photon notices one by observing a
version it has not seen before, so the record exists whether or not anyone wired up a pipeline; a build
system may enrich that record with a commit and an author, but never has to create it. A Deployment is
the first thing worth checking when a Service's numbers change shape, which is why it appears on the
charts rather than only in a list.

**Backend Issue** — a group of span errors sharing the same `(service, backend error fingerprint)`.
Identity is the pair, not the fingerprint alone: the same exception in two Services is two Issues,
because they have different owners and different fixes. Fingerprint is derived from the span's
exception event (type + digit-masked message + top frame), falling back to `status_message` when
the span carries no exception event. Compare [[rum-issue]] — the same idea on the Frontend world;
both use the same hash function (`photon_core::rum::fingerprint`), because two grouping rules for one
concept is how "the same error" stops meaning the same thing.

The fingerprint is computed at **ingest** and stored as the `error.fingerprint` span attribute, so
spans ingested before it shipped carry none and never will. There is deliberately no backfill and no
query-time fallback: one honest gap in the history beats two grouping rules for one concept.

**RUM Issue** — a group of browser errors sharing a `rum.error.fingerprint`, scoped to a RUM app.
The Frontend-world counterpart of a Backend Issue. Deliberately a separate concept: it is grouped
per app, not per Service, and its evidence is sessions rather than spans.

**Dependency** — a downstream call a Service makes, split by kind: a Database dependency (a CLIENT
span carrying `db.system`) or an External dependency (a CLIENT span naming a peer). A Dependency is
*not* a Service, even when the peer happens to be one.

**Caller** — a Service that sends work *to* another Service. The mirror of a [[dependency]], and a
different question: a Dependency answers "what am I waiting on", a Caller answers "who is hurt when I
break". Callers cannot be read off a Service's own spans the way Dependencies can, because the
evidence lives in the *other* Service's spans; they are recovered by following span parentage across
Services. A browser app instrumented for tracing appears as a Caller like any other.

**Edge** — one Caller-to-Service pair over the window, carrying its own volume, error rate, and
latency. The whole set of Edges is the fleet's shape. Read from one Service it is that Service's
Callers and Dependencies; read across all Services it is the map.

## Time

**Window** — the app-wide active time range. Every number on every Backend page is scoped to it.

**Previous Window** — the equally-long range immediately before the Window. The only comparison
basis for a trend chip; there is no baseline or seasonal model.

**Watch Range** — the short, deliberately bounded lookback the landing page judges over (minutes to
a few hours, never days). It is not the [[window]] and does not read it: a judgment of "wrong now"
averaged over a week is a lie, so the range that produces it is capped by design. Drilling out of
the landing page carries the Watch Range, so the page you land on shows the same slice of time.
