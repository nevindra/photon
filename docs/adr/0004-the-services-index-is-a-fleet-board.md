# The Services index is a fleet board, and the Site picker is only a filter

Status: accepted — decided, not yet implemented.
Implementation: [`docs/plans/2026-08-21-backend-services-revamp.md`](../plans/2026-08-21-backend-services-revamp.md)

`tenant` is a global context dimension: pick a Site in the ContextBar and every view narrows to it. That is
the right default for the explorers, where you are reading one Site's data. It is the wrong default for the
Services index, because the most useful question that page can answer is one you cannot ask from inside a
single Site: **is this service broken everywhere, or only there?**

`payments-api` unhealthy in three Sites out of three points at the code. Unhealthy in one out of three points
at that Site. `CONTEXT.md` already names this **Spread** and already says Services and RUM apps are the only
Subjects that merge across Sites, because they are the only ones that are one piece of software deployed many
times. The index had the vocabulary and could not use it.

So `/services` defaults to every Site, with rows merged per `(service, environment)` and a Spread badge,
expandable into one child row per Site. Selecting a Site in the ContextBar narrows the page and drops the
Spread column.

## The Site picker stays a filter, it is not ignored

The alternative was to have this page ignore the global selector outright, the way `/home` ignores the global
Window in favour of its own Watch Range. That works, but it required an ADR of its own to stop being
confusing, and it means a selection made on one page silently does nothing on another.

Treating the selector as an ordinary filter that is allowed to be empty gets the same behaviour with no
exception to the rule. "All Sites" is a legitimate value of the tenant dimension, not an override of it.

This costs nothing on a Site install: the tenant registry is empty, the Site band does not render, the Spread
column does not appear, and the page is an ordinary service table.

## A Site that stopped reporting must not read as healthy

Merging Sites into one page forces a problem the single-Site view could ignore. If `us-east` stopped pushing
fourteen minutes ago, its services report zero traffic — and `serviceStatus()` returned `idle` for zero
traffic, which renders as calm neutral grey. The page would have said nothing is wrong at the moment it was
blind.

ADR-0001 decided Silence is Down and never-reported is Unknown, and explicitly left the Services list alone
because "it is a different question". Once the list is a board across Sites, it is the same question. So:

- The Site band merges liveness from the tenant registry with per-Site RED, and marks a stale Site.
- Rows belonging to a stale Site are **Unknown**, badged `no data since HH:MM`. Never `0/s` and `OK`.
- `serviceHealth.ts` becomes the single Attention Level model for the Backend world, and `/home` imports it
  instead of re-deriving its own. `idle` is retired: it was one word covering two different facts.

## Consequences

- Cross-Site service health only works when Sites actually report their services. A `summary`-mode Site
  reports almost nothing about services today, which is why ADR-0002's rollup ships on the summary payload.
  Without that, this page would work only for Sites able to afford `full` mode.
- The index carries no column whose time scale differs from the active Window. SLO error budget (30 days) and
  data usage (cumulative) were both prototyped here and both removed: one row showing three time scales,
  none of them labelled, is a table that misleads confidently. Budget lives on the service Overview; usage
  lives on `/data`.
- There is no separate compare mode. Expanding a row already puts the Sites side by side in aligned columns,
  which is the comparison anyone actually wanted.
