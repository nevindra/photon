# Docker fleet management (deploy)

> **Status: built end to end.** The requirements are in the proposal this doc implements (a
> catalog of our services, push-deploy to customer sites without inbound access, and the
> telemetry to prove it worked, in one UI). Every screen an operator uses is wired through
> `lib/core/api.ts` to routes `photon-api` serves over `photon-deploy` (the pure domain) and
> photon-server's control-plane SQLite; a real `photon-agent` with the `docker` feature polls
> the server and drives the Docker daemon. See [Backend](#backend) below.
>
> **Availability is a deliberate switch, never a guess** (`lib/deploy/availability.ts`). A backend
> that predates the subsystem, or one started with `[deploy].enabled = false`, answers its routes
> with an *empty* 404 (a JSON-bodied 404 is a real answer). In **demo mode** — a dev build,
> `localStorage['photon-deploy-demo'] = '1'` (the "Preview with demo data" button), or no backend
> at all — the mock corpus (`lib/core/mockDeploy.ts`, loaded lazily so it never ships in the main
> chunk) stands in and the shell shows its "demo data" dot. Otherwise every deploy read throws
> `DeployUnavailableError` (never retried) and every Deploy surface renders `DeployUnavailable`
> — the Attention Board gains no Deploy world, `/infra` no agent strip, and a real install never
> sees a fake Critical stack on its landing page. Roles follow the same rule: the role picker and
> badges appear only when the session carried a `role` (`auth.ts` `rolesSupported`).
> `lib/core/api.deploy.test.ts` proves the switch outside a dev build.

Docker is a **capability of a Host**, not a new noun. Everything Docker lives under `/infra/:host`
or in the Deploy world; the vocabulary is in [`../../CONTEXT.md`](../../CONTEXT.md).

## Phases, and which are built

| Phase | Ships | UI status |
|---|---|---|
| 0. Foundations | roles, audit log, agent enrolment + tokens, SSH bootstrap, agent upgrades | **built** |
| 1. Read-only Docker | containers / images / volumes / networks per host, logs, stats, events | **built** |
| 2. Lifecycle | start / stop / restart / kill / remove / pull / recreate, prune, single host, audited | **built** |
| 3. Stacks | compose stacks: create, apply, destroy, edit, adopt; desired state and drift | **built** |
| 4. Templates | manifest, local registry, typed variables, render + dry run, import, observability block | **built** |
| 5. Fleet rollout | target selectors, staged rollouts, per-target status, rollback, pre-pull | **built** |
| 6. Tie-ins & polish | Attention Board Subjects for stacks and rollouts, the exec terminal | **built** (git-backed stacks, offline bundles and a two-person approval are not started) |

## Roles

`lib/deploy/roles.ts` is the whole model: `viewer` (today's implicit role — sees everything),
`operator` (container lifecycle, stacks and rollouts from approved templates, reconcile drift),
`admin` (everything an operator can, plus templates, agents, arbitrary compose, exec, users). The
signed-in user's role rides on `GET /api/session` (`role`) and lives in `lib/core/auth.ts` as
`role` + `can(permission)`. **A backend that predates roles reports none, and that reads as
`admin`**: the single implicit role of that era could press every button the UI had, and locking
an operator out of their own install on the day they upgrade is the wrong failure.

The UI never *hides* a control a role lacks. `components/deploy/GatedButton.vue` takes a
`perm` and, when the role falls short, renders the button disabled with a tooltip naming the role
it needs ("Requires the Operator role"). A button that vanishes reads as a missing feature; one
that refuses reads as a policy. Settings → Users shows each user's role as a badge and, for an
admin, a per-user role picker plus a role choice on the add-user form (`PATCH /api/users/:name`,
`POST /api/users` gains `role`).

## Managed agents

A **managed host** is a Host whose `photon-agent` was enrolled here with a Docker capability. The
row (`ManagedHost` in `api.ts`) carries what the agent *reports* — version, capabilities, Docker
facts, last sync — and what the server holds for it: Site, poll interval, labels, enrolment time.

Three display states (`lib/deploy/agentStatus.ts`): **Online**, **Offline** (reported, then
stopped — stated plainly, never as a failure of anything queued for it; desired state waits as
pending and applies when the agent returns), and **Waiting for first sync** (enrolled, never
called home — neutral, Photon does not know what runs there yet). Capabilities are `read` /
`lifecycle` / `apply` / `exec`, each granted by an agent-side flag (`--docker-read` …). They are
opt-in on the host: the server intersects what it wants with what the agent admits to, so the UI
shows them as reported and says a reinstall is how they change — no switch that would do nothing.

A managed host that reports **no resource metrics yet** (the agent runs without the metrics
sampler, or was installed a minute ago) is still a row of the `/infra` fleet table
(`HostFleetTable`, [`infra.md`](infra.md)): greyed, its status across the meter columns, a link
to its Agent tab — the page an operator enrolled it from must be able to find it, and a count
that disagrees with the list under it is a contradiction.

### Enrolling a host (`/infra` → Add host)

`components/infra/AddHostDialog.vue` opens on the one choice that decides everything else —
**two radio cards**, SSH or manual, each saying what it does — then shows only that path's
fields; the agent settings that rarely change (capabilities, poll interval, keep-credentials)
fold under an "Agent settings" row whose summary reads what they are ("read + lifecycle · every
15 s · credentials discarded after install"), so a default is read before it is accepted. The
Cancel / Install row is pinned below the scrolling form. The two paths:

- **Install over SSH** (default, R1.9): addresses (one per line — several enroll as one job), SSH
  user + port, password or private key, optional host name (single address only), Site (only when
  a tenant registry exists), capabilities (`CapabilityPicker`), poll interval (floor 15s), and a
  **Keep SSH credentials** switch that is off by default and explains both states — off means
  used once and discarded, on means this Photon becomes a key to every host. Submit starts
  `POST /api/agents/bootstrap` and the dialog turns into one stepper per host (connect → detect
  → push binary → write config with a fresh token → install and start → wait for first sync),
  polled once a second via `GET /api/agents/bootstrap/:id` until it settles. The headline counts
  ("1 of 2 hosts installed"); a failed host shows the step and the error and offers **Retry
  failed hosts** with just those addresses. The operator pastes nothing and never sees a token.
- **I'll install it myself**: host name + Site + capabilities + poll interval → `POST /api/agents`
  mints a per-host token, shown ONCE in `AgentTokenPanel`. The response also carries
  `server_url` (`[deploy].server_origin`, else derived from the request's `Host` +
  `X-Forwarded-Proto` — `agents::request_base`, shared with the sync upgrade URL) and
  `agent_archs` (which architectures `GET /api/agent/download` can answer for right now), so the
  panel never guesses either from the browser's origin — a Vite dev server on `:5173` is not
  where the sync endpoint lives (the dev proxy sets `changeOrigin` so the derived URL says
  `:8080`), and a snippet whose first line would 404 is worse than one that says so. The panel is
  three numbered steps, each naming what can go wrong BEFORE anything is pasted: **1** the token,
  copyable, with the shown-once warning; **2** the URL the host will dial, **editable**, flagged
  when loopback (only works on this machine), plain `http` (the token travels in clear) or when
  this Photon has no agent binary to hand out; **3** the command, **Install as a service**
  (systemd, every privileged line under `sudo`, `curl -fSsL` so a failure is printed instead of
  swallowed, ends with `systemctl status`) or **Run in the foreground** (no root, downloads into
  the current directory, Ctrl-C stops it — the "try it first" path). The token is left OUT of the
  command by default: `photon-agent` prompts for it (echo off) when stdin is a terminal, so it
  never lands in shell history; a checkbox embeds `--token ak_…` for pasting into an automation
  and the snippet then says to clear the history. The `docker run` format is gone — no
  `photon-agent` image is published, and a container has no `docker compose` for `--docker-apply`.
  Same panel idiom as the federation tenant dialog, because it is the same moment.

### The Agent tab (`/infra/:host/agent`)

`HostScaffold` (back button, `HostSubNav` Overview · Agent, Related menu) wraps every host tab, so
Overview never waits on an agent read. `HostAgentPanel`:

- not enrolled → an enrol prompt that opens the dialog on the manual path with the name filled;
- enrolled → Connection (status pill, last sync, poll interval, Site, enrolled), Agent version
  (with **Upgrade on next sync** when the server advertises a newer one — R1.10, upgrades ride
  the sync channel, not SSH), Capabilities as reported, Docker facts (daemon/API/driver/OS/arch,
  containers running/total, images, labels) with `DockerDiskBar` — images / volumes / build cache
  over the disk, the reclaimable slice called out — and the admin strip: poll interval, **Rotate
  token** (shown once, in the same three-step `AgentTokenPanel` — `POST …/rotate-key` returns
  `server_url` + `agent_archs` too), **Remove from management** behind a
  type-the-host-name confirmation that says what does *not* happen (containers keep running).

`/infra` itself gains the **Add host** action; enrolled agents that are offline or waiting for
their first sync are rows of its fleet table, ranked above the healthy hosts.

## Read-only Docker: the Containers tab (`/infra/:host/containers`)

`HostContainersPanel` reads the whole inventory in one poll (`GET /api/agents/:host/inventory`,
15s) and renders, top to bottom:

- **The health line** (`ContainerKpis`): running / total (+ compose-project count), unhealthy
  (health-check failures and crash loops — error accent only when non-zero), restarts, and Docker
  disk with a meter and the reclaimable slice, **Prune…** beside it — one line, not four tiles:
  four numbers that are usually "fine" do not earn a quarter of the screen.
- **A stale banner** when the agent is offline or has never synced: the LAST inventory the agent
  reported, labelled "this is what it saw 3d ago · nothing below is live", rather than an empty
  page. What was running before a box went quiet is still the best answer to "what runs there".
- **The inventory card**, whose header holds the view switch (`?view=` containers · images ·
  volumes · networks, with counts), the text filter (`?q=`) over name / image / project / label
  values, and "as of" — on the thing they control, not floating above it. The compose-project
  groups (or the images / volumes / networks table) stack inside it.
- **Containers grouped by compose project** (`ContainerGroup`, `lib/deploy/containers.ts`
  `groupByProject`): one panel per `com.docker.compose.project`, named projects first, standalone
  containers last, so a stack Photon deployed and one somebody `docker compose up`'d by hand look
  the same — the managed one carries a **Managed by Photon** chip. Inside a project the compose
  *service* is the primary name (the compose file's word), the container's own name under it in
  mono. Columns: state dot (the worse of state and health; a non-zero exit is an error) · service
  · image (registry stripped) · Docker's own status text + a health pill when not healthy · CPU
  · memory with a meter against the limit · restarts (warn > 0, error ≥ 5) · ports as chips — a
  **published TCP port is a link** that opens the service in a new tab (`portUrl`: on the host's
  name when bound to every address or loopback, on the bound address otherwise, `https` for 443),
  and its click does not also open the drawer. Rows stay flat; clicking opens the drawer.
- **Images / volumes / networks** tables (`ImagesTable` largest first with `unused` /
  `dangling` chips and per-image **pull** / **remove** actions (`image_pull` / `image_remove`
  Actions through `ImageActionDialog`; remove is offered only for an image no container uses
  and still asks once, since a 2 GB layer set on a slow customer link is a real loss),
  `VolumesTable` with "nothing — a prune would remove it" and a per-volume **remove** action
  (`volume_remove` through `VolumeRemoveDialog`: the `typed` tier, since a volume IS data —
  the name typed, the size stated; offered only for a volume no container mounts, running or
  not, and the server refuses the rest against the last inventory — `volume` rides on
  `ActionInput`/`ActionRecord`/`ActionSpec`/`AuditTarget`, the audit row is `volume.remove`,
  and the agent calls the Engine's volume delete WITHOUT `force`), `NetworksTable`).
- **Recent actions beside Docker events**, side by side under the card (the feed takes the
  whole row when Photon did nothing lately). `HostActionsStrip` is a list, one row per action
  with its outcome dot on the left and the agent's own words under the target, so a failed pull
  is the one red row and not a chip in the middle of a sentence. `DockerEventsFeed` shows create
  / start / die / OOM / health from the daemon, newest first, with an **Open in Logs** link — the
  same events ship as OTLP logs, which is how an OOM-kill reaches the Attention Board without
  anyone opening this page. A health check execs every ten seconds and Docker reports each exec
  as three events (exec_create / exec_start / exec_die), so on a quiet host the raw feed is one
  command repeating: `foldEvents` (`lib/deploy/containers.ts`) turns one actor's exec events
  into a single row — the command, the count, "every 10s" from the median gap between starts —
  that opens to the raw events on click, in the place its newest event held. Everything that is
  not an exec stays its own row.

The **container drawer** (`ContainerDrawer`, a Sheet; `?c=<name>` so it survives reload) has
three in-page tabs. **Logs** (`ContainerLogView`) loads a 200-line tail then, with *Follow* on
(default for a running container), polls "everything since the last line" once a second
(`useContainerLogs`) — no tunnel, no second port; the view pins to the bottom until the reader
scrolls up, after which a "↓ N new" chip offers the way back; stderr is tinted, a filter narrows
client-side, Wrap and Copy do what they say. **Stats** (`ContainerStatsView`) draws CPU, memory
(against its limit), network in/out and block I/O over the active Window off the `container.*`
metrics the agent ships as OTLP (`GET …/containers/:id/stats`). **Details** (`ContainerDetails`)
is `docker inspect` minus the secrets, built for the operator hunting ONE string in a hundred rows:
a sticky find box filters every section (Overview, Ports, Environment, Labels, Mounts) to the rows
whose key or value contains the query, case-insensitively, with each hit marked
(`docker/Highlight.vue` over the pure `lib/deploy/detailsSearch.ts`); a section with no hit folds
away and the box counts the matches. Rows are key ▸ value with keys in a fixed column, values that
wrap rather than truncate, env sorted by name, a copy button on hover for anything worth pasting,
and a bind mount's host path on its own line above its destination. Secret-looking env values
arrive masked from the agent and render as a lock + "masked" — they are never searchable.

`/infra`'s host cards carry a Docker chip (running / total) from the managed-agent list, so the
fleet grid says which hosts are under management.

API: `GET /api/agents/:host/inventory`, `GET /api/agents/:host/containers/:id/logs?tail&since`,
`GET /api/agents/:host/containers/:id/stats?start&end`, `GET /api/agents/:host/events?limit`.

## Lifecycle: one-shot Actions on one host

An **Action** is "do this once, soon" — a restart, a stop, a pull — as opposed to a Stack's
desired state. `lib/deploy/actions.ts` is the vocabulary: kinds (`start` `stop` `restart` `kill`
`pull` `recreate` `remove` `prune`), which apply to which container state (`availableActions`,
`primaryAction` — Restart for a running container, Start for a stopped one), and the
**confirmation policy** sized to the blast radius:

| risk | who | what the dialog does |
|---|---|---|
| `none` | start | nothing — it just runs |
| `confirm` | stop, restart, kill, pull, recreate | one sentence on what Docker will do (a stop names the ports that go dark), one button |
| `typed` | remove, prune-with-volumes, volume remove | the container's (or volume's) name, or `prune`, typed, with the data-loss line spelled out |

Every row carries the primary action plus a "more" menu (`ContainerActionMenu`); the drawer
header carries the same. Both are **disabled, never hidden**, when the role falls short
(`can('lifecycle')` → "Requires the Operator role") or the agent was installed without
`--docker-lifecycle` — the reason is the tooltip. An **offline host is not a "no"**: the confirm
dialog (`ActionConfirmDialog`) says the action waits as *pending* and runs on the next sync, or
expires after its 15-minute lease so a restart cannot land during tomorrow's peak.

While an action is queued or running the row's status cell shows "Restarting…" and the host's
**Recent actions** list (`HostActionsStrip`) carries a row per action (Queued / Running / Done
/ Failed / Expired, with the agent's result text). `useHostActions` polls every 2s while anything is in flight, and on settle
refreshes the inventory and audit log and raises one toast with the agent's result ("restarted
in 2.1s", or the registry's 404 for a failed pull). Every action is an audit row
(`container.restart` …, `prune`), `pending` for a queued one.

**Prune** (`PruneDialog`, on the Images and Volumes views) fetches a preview first — count and
bytes per category — and shows "Would free 2.8 GB (images 2.1 GB, build cache 0.7 GB)" before
anything is deleted. Images, stopped containers and build cache are on by default; **volumes are
off**, carry a data-loss line, and turn the confirmation into a typed one, because an unused
volume is sometimes last month's database.

API: `POST /api/agents/:host/actions` `{ kind, container_id?, args? }` → `{ ok, action }`,
`GET /api/agents/:host/actions?limit`, `GET /api/agents/:host/prune/preview`. Mutations under
`/api/agents` treat an *empty* 404 as "route not there yet" (mock fallback) and a JSON-bodied
404 / 400 / 403 / 409 as the refusal it is (`isRefusal` in `api.ts`).

## Stacks (`/deploy/stacks`, the Deploy world)

A **Stack** is one compose project on one host under **desired state**: the folder (compose file,
`.env`, sidecars) is the source of truth, every save is a new **generation**, and the agent
converges the host to the generation Photon last applied. The Arcane model, with Portainer's
word. Its status is the state machine from the proposal — Declared → Rendered → Pending →
Applying → Running → Drifted / Failed, Destroying → Gone — in `lib/deploy/stacks.ts`, and
**Pending is neutral, never red**: a host that has not answered is the normal case on-prem, and
anything sent to it waits rather than fails.

**The list** (`StacksView`, `StacksTable`): a strip of status chips that doubles as the filter
(all · needs attention = failed + drifted · pending · running — the Attention Board's world cards
in miniature; *Needs attention* keeps its `?attention=1` URL param, running / pending ride on
`?show=`), search and Host beside it, then the stacks **grouped under their hosts**
(`lib/deploy/stacks.ts` `groupStacksByHost`: feed it `sortStacks` output and a host is placed
by its worst stack, so the host with a failed stack leads the page). Each host row carries the
Site badge and the agent's state; each stack row is one line: status pill, name with the error
/ drift summary / "generation 13 waits for the next sync" under it (no separate Note column),
source (`template` + `nodeflux-vas 2.4.1` / `compose` / `adopted`, plus a "2.4.2 available"
badge when the template has a newer version than the stack runs), the compose services **by
name** each with its container's tone, the generation with the gap shown as `14 → 15` when a
newer one waits, and "Last change" in the status's own verb (`lastChangeLabel`: "apply failed
2h ago", "drift seen 14m ago", "applied 55m ago", "saved 5m ago by alfin"). Columns are bounded
so a wide screen does not spread them across the page. The Stacks entry is the first item of the new **Deploy**
world in the nav (Templates and Rollouts join it as they ship; `AppShell` resolves
`/deploy/<item>` to the nav key).

**New stack** (`NewStackDialog`) has two paths on one Segmented — **Write compose** (admin: the
one path that can run anything, R8.2) and **Adopt existing** (operator, R4.6: the compose
projects already running on a host that Photon does not manage, taken under management
*without recreating their containers*: the project's containers carry the `io.photon.stack`
label from the moment of adoption, so the new stack's Services grid shows them as running rather
than "missing on the host" — which is why adopting needs only the `read`
capability: an agent without `--docker-apply` can adopt, watch for drift, and is told that
applying changes from here is what needs the reinstall). The Segmented sits beside the title. The
compose path is a settings sheet — Stack name and Host as label-left / control-right rows (managed
hosts, with "no apply" / offline noted in the picker) — then `ComposeEditor` at full width, then
`DeployOptionsForm` — two groups: **Images**, the pull policy as three option cards
(missing / always / never), each saying what it does so "never" is never a guess; and
**Containers**, force recreate and recreate anonymous volumes as switch rows on one settings list,
the volumes row turning to the warn tint with its data-loss line when on — **folded** into a
`<details>` row whose summary shows the current values as badges,
since most stacks never change them. The *Apply right away* switch and the button (which says what
will happen to which host) sit in an action row pinned to the bottom of the dialog: the rest
scrolls, the actions do not.

**The editor** (`ComposeEditor`) is file tabs (compose file, `.env`, sidecars — add/remove;
paths must stay inside the stack folder, R4.2, Arcane's 10 MiB / 2000 entries / 20 levels caps
copied) over a mono textarea, with **Checks** in its status bar — beside the file name and the
character count, so they update where the eye already is and no side column sits empty next to
the text — that refresh as you type: services found, variables the compose references that
neither `.env` nor the secret store provides, and the **dangerous constructs** (`lib/deploy/compose.ts` `lintCompose`: `privileged`, the Docker
socket, host network, host PID, `cap_add`, bind mounts outside `/opt/photon` `/var/lib/photon`
`/srv` `/data`). Each warning has a stable id and its own acknowledgement box
(`ComposeWarnings`, R8.4); nothing saves or deploys until every one is ticked, and the ids are
stored on the stack so History can say what was acknowledged. Secrets are referenced as
`${secret:NAME}` and resolved on the agent at apply time (R5.9) — the editor says so.

**The detail page** (`StackDetailView`, `?tab=`): a header card — name, status pill (its hint
on hover), source, **one line of facts** (host → Containers tab, generation, last applied,
created by), the last error in full — and the action bar: **Apply** (turns brand when a rendered
generation waits: "Apply generation 16"), Restart, Redeploy (pull every image, recreate what
changed), and under ⋯ Take down and Destroy. Every verb goes through `StackActionDialog`, sized
like the container ones: apply / restart / redeploy state the effect and ask once; down asks
once; destroy wants the name typed and carries the named-volumes choice; an offline host is told
plainly that it queues. When drifted, **the drift is a row of that same header card**
(`StackDriftPanel`): one summary line with the two deliberate exits beside it — *Reconcile…*
reverts to the applied generation, its dialog listing exactly what will be reverted, because the
change was probably somebody keeping production up (R6.7); *Accept as desired…* renders a new
generation from what runs (audited as `stack.accept_drift`) — then one line per difference,
desired → observed, since when. Doing neither keeps the change and the record stays honest. Said
once, on the card whose status it explains, not four times across a pill, a hint, a banner
title and a banner subtitle.
Tabs: **Overview** — the compose services as observed, as **one table** (`StackServicesGrid`:
state dot · service · image · container linking into the host's Containers drawer · state ·
health · **drift**, the difference that concerns *this* service on its own row, tinted, so "lenz
is restarting" and "lenz.state drifted" are read together · **observe**: an APM link when the
compose service reports under that name, and a logs link on every row — by `service.name` when it
reports to APM, else the stack's logs narrowed by the container name). The header carries "This
stack's logs" and any APM service that shares no name with a compose service; the old
**Observability** block, which restated the same services as chips, is gone. Then **Did it
work?** (`DeployImpactPanel`, `lib/deploy/impact.ts` — the window is printed as an absolute stamp,
"15 min either side of 16:04", and a stack whose services sent no requests is **one line**, not a
card's worth of empty, instead of rows of `0.0/s → 0.0/s`: the stack's APM services 15 minutes
before the last apply against 15 minutes after — traffic, error rate, p99 per service and one
verdict, *Looks safe* / *Look closer* / *No signal yet* — from the same RED query the Backend
world uses; blunt thresholds, a prompt to look rather than an SLO). **Files** — the
editor over the stack's current generation with the deploy options; *Save as generation N+1*
renders without applying, the header's Apply button then offers the new generation. **History**
— the stack's own audit rows.

API: `GET /api/stacks`, `GET/PATCH /api/stacks/:id` (PATCH = files + acknowledgements + deploy
options → new generation), `POST /api/stacks`, `POST /api/stacks/:id/{up|down|restart|redeploy|
destroy|reconcile}`, `POST /api/stacks/validate`, `GET /api/agents/:host/compose-projects`,
`POST /api/stacks/adopt`. The list polls fast only while a stack is *applying* or *destroying*
— never for *pending*, which can last as long as a site is offline.

## Templates (`/deploy/templates`)

A **Template** is a compose file plus a manifest (`photon.template.yaml`, R5.1/R5.2): id,
version, name, description, icon, categories, **typed variables**, `requires` (arch, minimum
Docker, RAM, disk), the ports and volumes an operator confirms, a **health gate** (what
"deployed successfully" means — container health checks, an uptime monitor, or an SLO burn-rate
check: what Photon already has, never a fourth health concept, R7.4), and the **Photon block**
(R5.8: OTLP endpoint, ingest token, `service.name`, `deployment.environment` injected at render).
A **Template version is immutable** (R5.4): publishing is always a new version, and the detail
page's Deployments tab answers "which version is customer X on".

A **preset** variable (Portainer's `preset`: fixed by the template author, shown but not
editable) always takes the version's own default in `defaultVariables`, never the stack's stored
value — on an upgrade the stored value IS the previous version's pin, and letting it win made
the preflight diff show no image change while the old images would have shipped under the new
version's name.

Variable types (`lib/deploy/templates.ts`, `VARIABLE_TYPE_META`): `string` `number` `bool`
`select` `port` `path` `secret` `generated`, each with `required` / `default` / `label` /
`description` / optional `pattern` / `group`, plus Portainer's `preset` (fixed, shown not
editable). `VariableField` renders each as the right control — a secret is a write-only password
field that says it is stored encrypted, with a show/hide eye toggle (`data-testid="toggle-secret"`)
that reveals only what is being typed in that field, never a stored value (the server never hands
one back), a generated value is read-only ("Photon mints it"), a
preset shows a lock — and `validateVariables` is the one validator both the wizard and the
publish preview use. `VariablesForm` lays them out as a **settings sheet**: one section per
`group` (General first, Secrets last) under a real heading, and inside it one row per variable
with the label and its description on the left and the control on the right. That is what lets a
long hint (the uid-1000 warning on a database image) sit beside its own field instead of pushing
the next row out of line, and it keeps every control starting at the same x; identifier-shaped
values (port, number, select) get a short field, everything else a medium one.

**The catalog** (`TemplatesView`): a card grid (`TemplateCard`: icon, name, one line, categories,
latest version, versions, "on N hosts"), search, category chips, an Ours / Imported switch;
Deploy starts from the card. **Sources** (`RegistriesDialog`, admin): registries by URL (ours,
Portainer v2/v3 JSON) with sync / remove, a one-shot **Import Portainer JSON** paste with a
client-side preview (`lib/deploy/portainer.ts`: compose stacks and containers become templates
with typed variables; Swarm stacks are skipped — Photon does not cluster), and **Image
registries** — the credentials hosts pull *private images* with (see
[Image registry credentials](#image-registry-credentials) below). **New template /
Publish version** (`PublishVersionDialog`, admin): structured manifest fields (no YAML to
hand-write; the Manifest tab shows the `photon.template.yaml` the fields produce), the compose
file with a lint of risky constructs and a "declare them" shortcut for undeclared `${VARS}`, a
typed variables table, requirements, the Photon block and the health gate. Publishing onto an
existing template seeds from its latest version with the version bumped.

**The detail page** (`TemplateDetailView`, `?tab=`): Overview (what a host needs, and the
observability panel — what gets injected, and the health gate), Variables, Versions (each with
notes, deployed count, *Deploy this version*), Files (compose + generated manifest of the
selected version, `?v=`), Deployments (the stacks table filtered to this template).

**The deploy wizard** (`DeployTemplateDialog`), four steps because they are four questions:
**Target** (host — "no apply" / offline noted; stack name; version), **Variables** (the typed
form, grouped; errors for empty required fields appear only after an attempt to continue),
**Preflight** (R5.6 — `POST /api/templates/dry-run` renders and checks against the host's facts:
agent capability and reachability, arch, Docker version, memory, free disk, every port against
what the host publishes, images "checked by the agent at apply"; `PreflightList` blocks on a
`fail`, states a `warn`, admits a `skip`; then what will be minted / resolved on the agent, the
dangerous-construct acknowledgements, and a **line diff** of the rendered compose against what
the host runs now (`DiffView`, `lib/deploy/diff.ts`) with the rendered `.env` behind a
disclosure), **Deploy** (the deploy options, then a receipt: "Ready to deploy / upgrade" over the
three facts the earlier steps fixed — stack name, template + version (and the version it comes
from on an upgrade), host with its status dot and site — an offline host noted as "waits as
pending" in the receipt's header, and one sentence on what the host does next). The same wizard is the **upgrade** path: a template stack whose catalog has a newer
version shows "2.4.2 available" and an *Upgrade* button on its page, which opens the wizard
seeded with the stack's host, name and variables and diffs against its current generation. A
deploy whose host already runs a stack of that name **says so on the Target step** (a full-width
"This is an upgrade, not a new stack" banner under the three choices), seeds its variables and
diffs against it — never a silent replace. The Target step itself is the same settings sheet as
Variables — Host, Version, Stack name as label-left / control-right rows — followed by "What this
template needs" as a checklist marked against the chosen host's reported facts
(`lib/deploy/templates.ts` `requirementChecks`): arch, Docker version and free disk get a real
✓/✕ from the agent's `DockerFacts`, while RAM, ports and volumes stay neutral and say "checked at
preflight", because the host is what establishes those. No host, or no facts yet, is `unknown`,
never a fail — the list guides the choice, it does not gate it. Both wizards (`DeployTemplateDialog`,
`NewRolloutDialog`) pin their Back / Cancel / Next row to the bottom of the dialog: the steps scroll
inside, the actions do not, so a long Variables form never scrolls the buttons away. Stacks store the *rendered* compose, so that diff is what changes on the
host, not template placeholders against values.

API: `GET /api/templates`, `GET /api/templates/:id`, `GET /api/templates/versions/:id`,
`POST /api/templates/versions` (new immutable version), `GET/POST /api/registries`,
`POST /api/registries/:id/sync`, `DELETE /api/registries/:id`, `POST /api/templates/import`,
`POST /api/templates/dry-run`, `POST /api/templates/deploy` (→ a Stack; secrets travel in the
request once and are never read back), `GET/POST /api/image-registries`,
`PATCH/DELETE /api/image-registries/:id`.

### Image registry credentials

A template whose image lives on a private registry (`registry.gitlab.com/nodefluxio/lenz`,
`ghcr.io/org/app`) needs a credential on the host that pulls it, and Photon never `docker login`s
a host — that would leave a plaintext token in root's `~/.docker/config.json`, per host, outside
the audit log. Instead a credential is a server-side object: **registry host** (as an image
reference names it — `registry.gitlab.com`, `ghcr.io`, `docker.io`; `registry::normalize_server`
folds scheme, case and Hub's aliases), **username**, **token** (write-only: encrypted with the same
`SecretCipher` as stack secrets, never read back, rotated by replacing), an optional label, and a
**scope** — every host, or a `TargetSelector` (hosts / site / labels, the rollout selector reused)
naming which managed hosts may receive it. For GitLab the right token is a **Deploy Token** with
`read_registry` (project or group level; its username is the token's own), not a personal token.

Delivery is per pull, over the agent's own token channel: right before a stack apply or a
pulling action (`pull`, `image_pull`, `recreate` with pull) the agent calls
`POST /api/agent/registry-auth` (`{ servers }`, empty = all it is scoped for) and gets the
decrypted credentials for its host — one per registry, newest wins — holding them in memory for
that operation. Both pull paths use them: the **Engine API** path (bollard `create_image`) sends
`X-Registry-Auth` per request, because the daemon reads no `config.json` for API pulls; the
**compose** path writes a throwaway `DOCKER_CONFIG` directory (0700, under the agent's data dir)
whose `config.json` is the host's own config with our `auths` laid over it and the host's
`cli-plugins` dir added to `cliPluginsExtraDirs` (`docker compose` is itself a CLI plugin, and a
user-local install would otherwise vanish behind `DOCKER_CONFIG`), and removes it when the
apply ends. A `destroy` or `down` never pulls, so it never asks. A server that predates the route
answers 404, which the agent reads as "none" and pulls as before. **Preflight** names the outcome
per registry (`registry:<host>` checks): pass "as <user> (<label>)" when a credential is scoped
to the host, skip for anonymous Docker Hub, warn when a non-Hub registry has none — whether an
image is private is not knowable from the server, so a missing credential warns and never
blocks. Every create / rotate / delete is an audit row (`registry_credential.*`, registry + user,
never the token); the pull itself is audited as the action or stack result it belongs to.

## Rollouts (`/deploy/rollouts`)

A **Rollout** applies one template version to a set of targets, **staged**: a canary subset,
an explicit gate, then the rest (R6.2). `lib/deploy/rollouts.ts` holds the vocabulary and the
pure planning: `resolveTargets` turns a selector — one or more **hosts**, a **Site** (federation's
tenant, not a second grouping concept, R6.1), or a **label** match — into the managed hosts it
names, noting who waits (offline → pending) and who is out (agent without `--docker-apply`);
`planStages` picks the canary (count or percent, at least one, online hosts first so the gate can
actually be judged) and puts the rest in stage 2. Planning happens **after** preflight, in the
wizard and in the mock alike: a host that fails its dry-run is out of the plan before the
canary is chosen, so the canary is never a host that cannot take the version, the Review
summary counts only planned hosts ("1 host in 1 stage · 1 skipped"), and a rollout with no
host left passing preflight is blocked at Review rather than created empty. The Review names
each skipped host with its reason, and a secret every target's existing stack already holds
reads "Stored — leave blank to keep" instead of demanding re-entry on a site-wide upgrade.

Per-target statuses split two facts that share a word elsewhere: **`queued`** = its stage has not
been reached (nothing sent), **`pending`** = its stage is live but the host is not answering.
States (`ROLLOUT_STATE_META`): Draft → Running → **At the gate** (the canary finished; promote or
roll back) / **Paused — failures** (the stage's failure count crossed the threshold, R6.4; nothing
further is applied until an operator retries, resumes anyway, aborts, or rolls back) → Completed
/ Aborted / Rolled back. Per-target statuses (`TARGET_STATUS_META`): `pending` (waiting for the
host — **neutral, never red**), `prepulling` (R6.6: images fetched before the restart window),
`applying`, `healthy` (applied and the template's health gate passed), `failed`, `drifted`,
`skipped` (preflight said no; not counted as a failure).

**The list** (`RolloutsView`): a count band (needs a decision · active · completed · rolled
back), a filter (all / needs a decision / active / done, `?show=`), and rows sorted decision-first
with a per-target **progress bar** (`RolloutProgressBar`: healthy / in flight / failed / drifted /
pending / skipped segments).

**The detail page** (`RolloutDetailView`): header (state pill with its hint, the version, selector
and strategy summaries, what it upgrades *from*, progress), the verbs the state allows
(`availableRolloutActions`: start / promote / pause / resume / retry / abort / roll back), each
behind `RolloutActionDialog` — promote says how many hosts the next stage touches, abort says
what stays, **rollback** says it re-applies the previous immutable version with the previous
variables to the hosts this rollout touched, an ordinary apply and not a separate mechanism
(R6.5). Then the **stage board** (`RolloutStageBoard`): one panel per stage with its target rows
(host, Site, status, note or error in full, attempts, a link to the stack), the current stage
outlined, and the **gate drawn between stages** — locked and amber while waiting for the
operator. Skipped targets sit in a dashed panel of their own. Then the rollout's audit rows.

**The wizard** (`NewRolloutDialog`, from the Rollouts page, a template's *Roll out to many*, or
the version matrix's "roll out to the N hosts behind"): Template (and version) → Targets
(selector kind, the picker, the stack name; a live *Resolved targets* panel says who is in, who
waits, who is out) → Variables (the typed form, same values to every target; generated secrets
are minted per host) → Strategy (canary size, failure threshold, pre-pull, auto-promote — off by
default, an operator promotes after looking at the canary — the health grace, and the compose
options: pull policy / force recreate / recreate volumes, R4.3) → Review: the stage plan with a
**per-target preflight** (the same dry-run the single deploy gets, once per host — a taken port
or a full disk is known before anything ships and that host is planned as skipped), and the
version's **dangerous constructs**, each needing its acknowledgement before *Create and start*
enables (R8.4 on the fleet path too).

**At the gate** the detail page adds `CanaryVerdictPanel`: the canary hosts' services judged the
same way a single deploy is — promote on evidence, not on a green apply (R7.4).

API: `GET /api/rollouts`, `GET /api/rollouts/:id`, `POST /api/rollouts`,
`POST /api/rollouts/:id/{start|promote|pause|resume|retry|abort|rollback}`. The mock supervisor
is time-driven (a target on an online host moves pending → prepulling → applying → healthy in a
few seconds; a host whose name contains `fail` fails its apply, which is how the threshold path
can be demonstrated). The list polls fast only while a rollout is *running*.

## The Attention Board tie-in

`lib/home/attention.ts` grows a **Deploy** world (between Infra and Uptime, only while something is
under management) and two Subject kinds: `stackSubject` — a **failed** apply is Critical, a
**drifted** stack is Warning, running is OK, pending is *not* a level (an unanswered host is the
normal case; a rendered-not-applied generation is the operator's own choice), gone is history;
`rolloutSubject` — **paused on failures** is Critical, **at the gate** is Warning, running is OK,
finished rollouts are history. No new severity vocabulary: a failed rollout is just another
Subject at Critical, exactly as the proposal argued. Both carry their Site, so a tenant lens
narrows them, and `useAttentionBoard` gives the Deploy world its own vital signs (stacks running,
drifted, failed applies, rollouts in flight).

## Exec: a shell in a container (R3.4)

The single highest-risk feature, shipped last and audited. The **Shell** tab of the container
drawer (`ContainerShell`) is gated three ways — the **admin** role, the agent's own `--docker-exec`
capability, and a live agent — and each refusal is stated in the tab rather than hidden.
Opening a session writes an audit row; so does closing it, with the duration and command count.
The stream is agent-initiated (`POST /api/agent/stream/:session` outbound), so no port opens on
the host. Secret-looking environment values (`pass`, `secret`, `token`, `key`, `licen…`) come
back masked. Leaving the tab ends the session — a shell nobody is looking at is a shell nobody
should hold. API: `POST /api/agents/:host/containers/:id/exec` → session,
`POST /api/exec/:id/input`, `DELETE /api/exec/:id`.

## Audit log (`/audit`)

Every mutation Photon makes to a host is a row: who (actor + role), what (`noun.verb` action,
rendered as a phrase by `lib/deploy/audit.ts`), where (host + the container / stack / rollout /
template it was about), when, and how it ended — `ok` / `failed` (the agent tried) / `denied`
(the role check said no; nothing ran) / `pending` (recorded; the host applies it on its next
sync). A row expands into its detail line, the payload **digest** (the rendered payload is stored,
never logged — secrets stay out of the trail, R5.9) and a link to the host.

The page deliberately does **not** follow the global time Window (`:time-picker="false"`): it is
control-plane history, not telemetry, and a 30-minute default would show an empty page on a quiet
fleet. It has its own coarse *Since* picker (24h / 7d / 30d / all) and the whole filter set
(`since`, `actor`, `act`, `host`, `result`, `q`) lives in URL params via `useUrlParam`, so a row
can be linked. Readable by every role.

## Routes, files, API

| Route | View | Notes |
|---|---|---|
| `/infra/:host/agent` | `InfraHostAgentView.vue` | the Agent tab; `/infra/:host` is the Overview |
| `/infra/:host/containers` | `InfraHostContainersView.vue` | the Containers tab (read-only Docker) |
| `/deploy/templates` · `/deploy/templates/:id` | `TemplatesView.vue` · `TemplateDetailView.vue` | the catalog; `/deploy` redirects here |
| `/deploy/stacks` · `/deploy/stacks/:id` | `StacksView.vue` · `StackDetailView.vue` | the Deploy world; lazy |
| `/deploy/rollouts` · `/deploy/rollouts/:id` | `RolloutsView.vue` · `RolloutDetailView.vue` | staged rollouts |
| `/audit` | `AuditView.vue` | Manage group, lazy |

Frontend: `lib/deploy/` (`roles.ts`, `agentStatus.ts`, `audit.ts` — pure; `deployQueries.ts` —
the TanStack composables, mutations never reject), `lib/core/mockDeploy.ts` (the corpus, with a
time-driven bootstrap simulation: an address containing `fail` refuses SSH, one containing `slow`
never syncs), `components/deploy/` (`GatedButton`, `AuditTable`, and the stacks pieces `StacksTable`,
`StackStatusPill`, `NewStackDialog`, `ComposeEditor`, `ComposeWarnings`, `DeployOptionsForm`,
`StackActionDialog`, `StackServicesGrid`, `StackDriftPanel`; the templates pieces `TemplateCard`,
`TemplateIcon`, `VariableField`, `VariablesForm`, `DeployTemplateDialog`, `PreflightList`, `DiffView`,
`RegistriesDialog`, `PublishVersionDialog`; the rollouts pieces `RolloutStatePill`, `TargetStatusPill`,
`RolloutProgressBar`, `RolloutStageBoard`, `RolloutActionDialog`, `NewRolloutDialog`; pure helpers in
`lib/deploy/stacks.ts`, `lib/deploy/compose.ts`, `lib/deploy/templates.ts`, `lib/deploy/diff.ts`,
`lib/deploy/portainer.ts`, `lib/deploy/rollouts.ts`), `components/docker/` (the Containers tab:
`HostContainersPanel`, `ContainerKpis`, `ContainerGroup`, `ImagesTable`, `VolumesTable`,
`NetworksTable`, `DockerEventsFeed`, `ContainerDrawer`, `ContainerLogView`, `ContainerStatsView`,
`ContainerDetails`, `ContainerShell`, and the lifecycle pieces `ContainerActionMenu`, `ActionConfirmDialog`, `PruneDialog`,
`HostActionsStrip`; pure helpers in `lib/deploy/containers.ts` and `lib/deploy/actions.ts`), `components/infra/`
(`HostScaffold`, `HostSubNav`, `HostAgentPanel`, `AddHostDialog`, `CapabilityPicker`,
`AgentTokenPanel`, `DockerDiskBar`), `ui/textarea`.

API surface (session cookie + role check; `photon-api/src/deploy/*.rs`): `GET /api/agents`,
`GET/PATCH/DELETE /api/agents/:host`, `POST /api/agents` (enrol → token, shown once),
`POST /api/agents/:host/rotate-key`, `POST /api/agents/:host/upgrade`,
`POST /api/agents/bootstrap` + `GET /api/agents/bootstrap/:id`, `GET /api/agents/:host/inventory`,
`GET /api/agents/:host/containers/:id/logs|stats`, `POST /api/agents/:host/containers/:id/exec`,
`GET /api/agents/:host/events|prune/preview|compose-projects`, `GET/POST /api/agents/:host/actions`,
`POST /api/exec/:id/input`, `DELETE /api/exec/:id`, `GET /api/audit?since&actor&action&host&result&q&limit`,
`GET/POST /api/stacks`, `POST /api/stacks/validate|adopt`, `GET/PATCH /api/stacks/:id`,
`POST /api/stacks/:id/{up|down|restart|redeploy|destroy|reconcile|accept}`, `GET /api/templates`,
`GET /api/templates/:id`, `GET/POST /api/templates/versions[/:id]`, `POST /api/templates/import|dry-run|deploy`,
`GET/POST /api/registries`, `DELETE /api/registries/:id`, `POST /api/registries/:id/sync`,
`GET/POST /api/image-registries`, `PATCH/DELETE /api/image-registries/:id`,
`GET/POST /api/rollouts`, `GET /api/rollouts/:id`,
`POST /api/rollouts/:id/{start|promote|pause|resume|retry|abort|rollback}`, `PATCH /api/users/:name`
(`role`). Mutations answer `{ ok, … }`; a refused one is a 400/403/404/409 with `{ error }`, and a 403
is worded as the role refusal it is ("Requires the Operator role"). `GET /api/agents/:host` answers
an unenrolled host with a JSON 404. Open routes: `GET /api/agent/download?arch=` (the agent binary).
Agent-token routes: `POST /api/agent/sync`, `POST /api/agent/secrets`, `POST /api/agent/registry-auth`,
`POST /api/agent/stream/:session`, `GET /api/agent/stream/:session/input`.

The agent token is a **fourth independent auth system** (see [`auth.md`](auth.md)): per host,
minted here, rotatable, never `[ingest].token` — an ingest leak costs fake telemetry, an agent
leak costs the customer their machine.

## Backend

**Crates.** `photon-deploy` (pure, depends only on `photon-core`): `model` (every type above, serialized
exactly as `api.ts` reads it), `proto` (the sync wire types, shared byte-for-byte with the agent —
`photon-agent` compiles this crate with default features off for just those two), `compose` (lint of
dangerous constructs with the same stable ids the frontend computes, service/image/variable parsing,
path safety, `validate_stack_files`), `render` (manifest validation, `default_variables` — a `preset`
always takes the version's default, `validate_variables`, `render` → compose + `.env` + the Photon
block), `diff` (desired vs observed → `DriftItem`s, `observed_services`), `preflight` (the checks the
wizard lists, verbatim), `plan` (`resolve_targets`, `canary_size`, `plan_stages`), `portainer`
(v2/v3 import), `ids` (`new_id`, `mint_token`, `sha256_hex`, `payload_digest`), and `store`
(`DeployStore`, the persistence trait, plus `MemStore` for tests). `photon-api/src/deploy/` is the
HTTP layer: `mod.rs` (`DeployApi`, the `SecretCipher` and `Bootstrapper` seams, audit helpers),
`agents.rs`, `inventory.rs` (inventory, logs, stats, events, prune preview, compose projects, exec),
`actions.rs`, `stacks.rs`, `templates.rs` (+ registries), `image_registries.rs`, `rollouts.rs`, `audit.rs`, `sync.rs` (the
agent endpoints), `hub.rs` (in-memory: per-host long-poll wake-ups, stream sessions, event and stat
rings, bootstrap jobs), `supervisor.rs` (the tick). `photon-server/src/deploy/`: `store.rs`
(`SqliteDeployStore` — document tables `deploy_*` in the shared control-plane DB, whole rows as
JSON with a few indexed columns), `cipher.rs` (AES-256-GCM under `deploy.key` beside the DB, R8.5),
`bootstrap.rs` (the SSH installer, R1.9). `main.rs` wires them (`spawn_deploy`) and spawns the
supervisor; `[deploy]` in `photon-core` config (`enabled`, `server_origin`, `ingest_origin`,
`secret_key_file`, `agent_binaries_dir`, `ssh_bootstrap`; `PHOTON_DEPLOY_*` env overrides).

**Auth.** UI users carry a `role` column (`viewer` / `operator` / `admin`; rows that predate roles
read as `admin`; the first account is the admin; new users default to `viewer`; the last admin cannot
be demoted). `require_auth` resolves the user once and hands handlers a `CurrentUser` extractor;
`CurrentUser::require(Permission)` / `deploy::require` are the 403 gate. The agent token is the
fourth system: `ak_` + 32 hex chars of OS entropy, only its sha256 stored (`deploy_hosts.token_hash`),
presented as `Authorization: Bearer` on the `/api/agent/*` routes, compared in constant time.

**The control channel.** `POST /api/agent/sync` folds the request into the store (identity,
capabilities as REPORTED, Docker facts, the observed inventory, events into the hub ring, action and
stack results, drift for every stack on the host), then answers with the desired document when the
agent's `known_generation` is behind the host's `desired_generation`, the open actions (queued →
running as they are handed out), the stream sessions the hub wants, an `upgrade` when one was
requested and a binary exists, `next_poll_secs` (1 while a stream is open), and the `ingest` hint. It
is a **long poll**: with nothing to send it holds the response up to `wait_secs` (≤ 60) until the
hub wakes the host (a new generation, an action, a stream), so a button press reaches an idle agent in
milliseconds without a faster poll or a second port (R1.6). Secrets never ride in `desired`: the
rendered `.env` lists their names, the agent fetches values from `POST /api/agent/secrets` at apply
time, and the store only ever holds ciphertext; image-registry credentials travel the same way
(`POST /api/agent/registry-auth`, per pull, see above). Streams are agent-initiated uploads
(`POST /api/agent/stream/:session`, NDJSON `StreamChunk`s) into hub buffers the UI reads; a session
nobody reads for 30 s (10 min for exec) closes and the agent stops.

**The host row is edited, never overwritten.** Two writers touch `deploy_hosts`: the sync fold (agent
version, capabilities, Docker facts, last-seen) and the UI (labels, site, poll, upgrade, the
`desired_generation` bump every stack write makes). A read-modify-write from either side's copy
loses the other's change — the observed case was a destroy whose generation bump landed between a
sync's read and its write, so the agent (which only asks "is the generation newer than mine") was
never handed the destroy and the stack sat in `destroying`. So `DeployStore::update_host(host,
closure)` edits under the store's lock and every live write goes through it; `put_host` is for
creation and never lowers `desired_generation`. Belt and braces, the sync handler also heals a
stack with unapplied desired state whose document the agent already holds (`heal_stale_generation`)
by bumping the host, so a store restored from backup recovers too. Wake-ups (`hub.wake`, the exec
input/output notifies) are `notify_one`, which stores a permit when nobody is waiting: a wake that
fires between two agent polls is honoured by the next one instead of lost, and every waiter
re-checks for work before waiting again, so a stale permit costs one look, never a wrong answer.

**The shell** (`POST …/containers/:id/exec`) runs `sh` in the container WITHOUT a pty: the UI is a
line-at-a-time console that prints the command itself, and a pty makes busybox `ash` start its
line editor, which queries the terminal for the cursor position and stalls waiting for an answer
nothing sends. `POST /api/exec/:id/input` waits up to 1.5 s for output that lands after the sent
line and returns everything not yet delivered (a per-session cursor, so late output is never
skipped) with `answered` saying whether the sent line's own output arrived; an empty line is a
poll, and the UI polls a few times while `answered` is false. The first command of a session takes
a few seconds (the agent learns of the session on its next sync, then starts the exec); the rest
answer at once.

**Long work never silences the agent.** Stack applies and one-shot actions run on background
tasks in the agent (`sync.rs`: a `JoinSet` of jobs, one apply per stack at a time); the sync loop
keeps polling every few seconds while a job runs, so the host stays `online`, logs and the shell
keep working, and the stack sits in `applying` until the result arrives. Before this, a 17-minute
image pull showed the host offline for 17 minutes. A document with a stack whose previous apply is
still running is not marked known, so the server re-sends it and the stack is picked up when the
job ends. A failed apply keeps the tail of the compose transcript as `last_error`, not just the
exit status. **Recreate volumes** means named volumes: the agent runs `down --volumes` and then
`up` (compose's `up -V` only renews anonymous volumes — a stack upgraded from PostgreSQL 16 to 17
kept its PG16 data under `-V` and crash-looped). An optional (`required: false`) secret variable
may be left empty and renders empty. The Photon block writes both the OTel spellings
(`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`) and the plain pair our own images
read (`PHOTON_OTLP_HTTP_ENDPOINT`, `PHOTON_OTLP_TOKEN`). The SSH bootstrap uses `sudo -S` fed the
SSH password when the account's sudo wants one (the common shape of a customer's ops account);
key auth on such an account is refused with a message saying so.

**Stacks, drift, rollouts.** A write bumps the stack's `desired_generation` and the host's document
generation. The agent reports `StackResult`s; success records `applied_generation`, the compose
config hashes as the drift baseline, and (for a template stack) a release marker in `deployments`
(R7.1); failure records `last_error`. Every sync re-judges drift (`diff::compute`) for running
stacks. The supervisor (3 s tick) expires stale actions, closes idle streams, and advances rollouts:
queued targets of the live stage get their stack applied through the same `deploy_template` path the
single deploy uses, in-flight targets read their stack's fate, a target turns healthy once its
containers ran for `health_grace_secs`, a stage settles into the gate (or auto-promotes), the
threshold pauses it, and rollback re-applies each stack's previous snapshot.

**The agent** (`crates/photon-agent`, `docker` feature default-on, bollard for the Engine API and the
`docker compose` CLI for stacks): `--server` + `--token` enable the control channel; `--docker-read /
--docker-lifecycle / --docker-apply / --docker-exec` are the whole authority (each refusal is local);
`--poll` (≥ 5 s), `--data-dir` (state + stack folders), `--docker-host`, `--no-metrics`; `install` /
`uninstall` / `version` subcommands (systemd unit + `/etc/photon-agent/agent.env`; `install` checks
for root FIRST and says "re-run with sudo" instead of three permission-denied lines, and — like a
foreground run with `--server` — prompts for a missing `--token` with echo off when stdin is a
terminal (`prompt.rs`), erroring with "pass --token or PHOTON_AGENT_TOKEN" when it is not, so the
SSH bootstrap and scripts are unaffected); `--dry-inventory`
prints the observed document. It keeps `state.json` (known generation, applied stacks, seen action
ids), re-bases its timestamps on the server clock (R9.3), self-upgrades from the sync response
(R1.10), and uses the server's `ingest` hint for its metrics when `--endpoint` was not given.
`crates/photon-server/tests/deploy_e2e.rs` drives the whole protocol with a fake agent.
