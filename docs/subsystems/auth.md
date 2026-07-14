# Auth & onboarding

Photon has **three independent auth systems** — never conflate them (see
[`../conventions.md`](../conventions.md)).

| System | Who | Mechanism | Config |
|---|---|---|---|
| **OTLP ingest** | services / collectors sending data | shared service **bearer token** | `[ingest].token` |
| **UI users** | humans in the browser | **argon2 password + signed session cookie** | `[auth].session_secret` (≥ 32 bytes) |
| **RUM beacon** | browsers posting `POST /api/rum` | per-app **public key + Origin allowlist** (only CORS-enabled, unauthenticated route) | UI-managed `rum_apps` registry (SQLite) — see [`rum.md`](rum.md) |

## UI users & onboarding

UI users live in the **SQLite control-plane DB** (`[storage].db_path`), **not** in config. First run:
open the UI and complete the one-time **"create your account"** onboarding (`POST /api/setup`); manage
additional users from the in-app **Settings** dialog. `photon-server hash-password '<pw>'` prints an
argon2 hash if you need one directly.

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
| `GET/POST /api/users` | session | list / create users |
| `DELETE /api/users/:username` | session | remove a user |

Handler: `crates/photon-api/src/{auth,users}.rs`. The `UserStore` trait is the DB seam.

## UI

- `/login` → `LoginView.vue`; `/onboarding` → `OnboardingView.vue`.
- Auth state (`frontend/src/lib/auth.js`): Pinia-free reactive refs `authed` / `username` /
  `needsSetup` + a cached `hydrate()` boot probe (httpOnly session cookie). The router `beforeEach`
  guard (`router/index.js`) gates every route on these — onboarding-first when no user exists, then
  login gating. User-management mutations (`frontend/src/lib/usersQueries.js`) return `{ ok, error }`.
