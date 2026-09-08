# Auth & onboarding

Photon has **three independent auth systems** — never conflate them (see
[`../conventions.md`](../conventions.md)).

| System | Who | Mechanism | Config |
|---|---|---|---|
| **OTLP ingest** | services / collectors sending data | shared service **bearer token** | `[ingest].token` |
| **UI users** | humans in the browser | **argon2 password + signed session cookie** | `[auth].session_secret` (≥ 32 bytes) |
| **RUM beacon** | browsers posting `POST /api/rum` | per-app **public key + Origin allowlist** (only CORS-enabled, unauthenticated route) | UI-managed `rum_apps` registry (SQLite) — see [`rum.md`](rum.md) |
| **Agent token** | `photon-agent` on a managed host polling `/api/agent/*` | per-host **bearer token** `ak_…`, minted at enrolment, sha256 stored, rotatable | UI-managed `deploy_hosts` (SQLite) — see [`deploy.md`](deploy.md) |

## UI users & onboarding

UI users live in the **SQLite control-plane DB** (`[storage].db_path`), **not** in config. First run:
open the UI and complete the one-time **"create your account"** onboarding (`POST /api/setup`); manage
additional users on the **`/settings/users`** page (the rail's gear; `frontend/src/views/UsersView.vue`
over `lib/core/usersQueries.ts`). Adding is a modal off the header's "Add a user" button
(`components/settings/AddUserDialog.vue`); the password is typed twice (`PasswordFields.vue`, shared
with the change-password dialog) because Photon never shows it again. Each row carries a key that
opens `ChangePasswordDialog.vue` — your own row asks for the current password whatever your role, an
admin resets anyone else's without it — and the table is paged client-side (`paginate` in
`lib/core/users.ts`, 10/25/50 per page, the strip mounted only once the roster could need it). The
page restates the server's rules before the round trip
(`lib/core/users.ts`: username non-empty, ≤ 64 chars, no spaces; password ≥ 8 chars, confirmation
must match) and explains
the anti-lockout refusals in place — you cannot remove yourself or the last user, and the last admin
cannot be demoted — instead of toasting them after the fact. `photon-server hash-password '<pw>'`
prints an argon2 hash if you need one directly.

## RUM beacon auth

The Origin allowlist backing `POST /api/rum` is no longer static config — it comes from the mutable
`rum_apps` store (`RumAppStore`/`SqliteRumAppStore`), fronted by a live in-memory cache that the
beacon handler and the CORS layer both read (see [`rum.md`](rum.md) for the CRUD API and cache
details). The public `key` is minted **server-side** (`pk_live_<uuid>`) when an app is registered or
rotated — operators never hand-pick it. Because the allowlist is live, a newly-registered origin
starts working immediately, no server restart required; an unregistered app now gets a **403** (RUM
has no "disabled" state to fall back to a 404).

## API

| Route | Auth | Purpose |
|---|---|---|
| `POST /api/login` | open | sign in |
| `POST /api/setup` | open (first-run) | create the first user |
| `GET /api/session` | open | boot probe (restores session on refresh) |
| `POST /api/logout` | session | sign out |
| `GET/POST /api/users` | session (create: admin) | list / create users (`role`, default `viewer`) |
| `PATCH /api/users/:username` | admin | change a role (`{ role }`); the last admin cannot be demoted |
| `DELETE /api/users/:username` | admin | remove a user |
| `PUT /api/users/:username/password` | session | set a password (`{ password, current_password? }`). Your own needs `current_password` (any role); someone else's needs admin and takes none — a reset is for a forgotten password. 204 / 400 (wrong current, too short) / 403 / 404. Sessions are stateless signed cookies, so an open sign-in survives the change |

Handler: `crates/photon-api/src/{auth,users}.rs`. The `UserStore` trait is the DB seam.

## UI

- `/login` → `LoginView.vue`; `/onboarding` → `OnboardingView.vue`.
- Auth state (`frontend/src/lib/auth.js`): Pinia-free reactive refs `authed` / `username` /
  `needsSetup` + a cached `hydrate()` boot probe (httpOnly session cookie). The router `beforeEach`
  guard (`router/index.js`) gates every route on these — onboarding-first when no user exists, then
  login gating. User-management mutations (`frontend/src/lib/core/usersQueries.ts`) return `{ ok, error }`.

## Roles and the agent token

Docker fleet management ([`deploy.md`](deploy.md)) adds two things here. **Roles** on UI users —
`viewer` / `operator` / `admin`, a `role` column on `users` (rows that predate it read as `admin`,
the first account is the admin, new users default to `viewer`), carried as `role` on `GET /api/session`,
on the login response, and on each row of `GET /api/users`. `require_auth` resolves the user once and
inserts a `CurrentUser { username, role }` into the request; handlers extract it, and
`CurrentUser::require(Permission)` is the 403 gate (`photon_deploy::model::Role::required` maps each
permission to the least role — lifecycle and deploy need `operator`; templates, agents, arbitrary
compose, exec and users need `admin`; the audit log is readable by every role). And the **agent
token**: a fourth independent auth system, per host, minted by `POST /api/agents` (or by the SSH
bootstrap, in which case nobody ever sees it), rotatable via `POST /api/agents/:host/rotate-key`,
presented as `Authorization: Bearer ak_…` on the `/api/agent/*` routes and compared in constant time
against its stored sha256. Never `[ingest].token`: an ingest leak costs fake telemetry, an agent-token
leak costs the customer their machine.
