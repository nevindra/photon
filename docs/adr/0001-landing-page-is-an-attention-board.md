# The landing page is an attention board, not a dashboard

Status: accepted

`/home` used to answer "what are the numbers?" — a KPI strip, two charts picked from whichever
service happened to be busiest, and three per-world panels. Somebody watching the fleet had to read
readings and do the judging themselves, and alert incidents did not appear on the page at all even
though `photon-alerts` was fully wired. We rebuilt it around one question instead: **is anything
wrong, and where?** Every watched thing is judged into a single ranked **Attention Level**, and the
page leads with the things that failed the judgment.

The vocabulary this rests on (Subject, Attention Level, Silence, Watch Range, Spread, Unattributed
Incident, Site, Central) lives in [`CONTEXT.md`](../../CONTEXT.md). This ADR only records the
decisions that a future reader would otherwise want to undo.

## The decisions

**One Attention Level across every signal.** Down / Critical / Warning / OK, plus Unknown — which is
not a level but the absence of a judgment. Each kind keeps its own native word (`degraded`, `poor`,
`down`, `error`) verbatim in the reason line; only the *ranking* is unified. Without a shared axis a
Host and a RUM app cannot be put in the same ordered list, and an ordered list is the entire point
of the page. A slow Web Vital is Warning and never Critical: a landing page that cries wolf about
latency stops being read.

**The Watch Range is not the global Window.** The page judges over its own short lookback (5m / 15m
/ 1h / 6h, default 15m) held in `lib/home/watchRange.ts`, and `AppShell` hides the global
`TimeRangePicker` for this one view. A judgment of "wrong now" averaged over seven days is a lie, so
the range that produces it is capped by design rather than by convention. Drilling out writes the
Watch Range into the global time range first (`setTimeRange`), so the page you land on shows the
same slice of time — the cap applies to the judging, not to the exploring.

**Silence is Down; never-reported is Unknown.** A Subject that reported inside the Watch Range and
then stopped is Down, because the stopping is itself the news. This closes a real hole: `serviceStatus()`
returns `idle` when `count` is 0, so a service falling from 5,000 req/s to nothing used to become
invisible exactly when it mattered. A Subject with no data at either end is Unknown — Photon cannot
tell a dead machine from an uninstalled agent, so it reports which one it observed instead of
guessing. Detection is client-side and needs no backend change: hosts carry `lastSeenNs`, services
and RUM apps are compared against the previous equal-length range.

**The roll-up cards are the filter; there are no tabs.** One card per World shows every Subject it
watches and the per-level tallies, and clicking it narrows the attention list and the vital signs in
place (`?world=backend`, click again to release). Tabs were the first design and were dropped after
a mockup: tabs hide the tallies of every world you are not looking at, which is precisely the
information that makes the page trustworthy when nothing is wrong.

**Only Services and RUM apps merge across Sites.** On Central a Subject is identified by
`(Site, kind, name)`. Services and RUM apps are one piece of software deployed many times, so
`payments-api` in three Sites is one row carrying a **Spread** ("3 of 3" points at the code, "1 of 3"
points at that Site). Hosts and Monitors never merge: `web-03` in `eu-west` and `web-03` in `us-east`
are two machines that happen to share a hostname, and merging them would invent a thing that does
not exist.

**Spread ships for Services only, for now.** `RedRow.tenant` is populated for `group=service`, so
service rows arrive already split per Site and merging them costs nothing. The RUM app registry has
no equivalent: with no tenant selected, `GET /api/rum/apps` returns Central's own apps, and a site's
apps are only reachable one tenant at a time via `?tenant=`. Cross-site RUM Spread would cost one
app query plus one vitals fan-out per Site, so it is deliberately left out rather than paid for.

**Unattributed incidents are shown, never hidden.** A triggered incident whose rule is genuinely
about the fleet (a metrics rule with no `group_by`) or whose place is not recoverable from the rule
resolves to no Subject. It gets its own banner above the roll-up. Hiding it would make the page lie
in the one case where the page is most needed. The inverse is allowed too: one incident may resolve
to several Subjects and appears on each.

**The page is the same code on Central and on a Site.** The difference is only in the data — on
Central every Subject carries a Site and a fifth World (Sites) appears; on a Site install none do.
A site team reads their own Site's Photon, so scoping falls out of what the data already is. This
avoids inventing RBAC, which is explicitly out of scope.

## Consequences

- `serviceStatus()`'s `idle` no longer reaches the landing page unchanged: the board re-reads it
  against the previous window before deciding between Down and Unknown. The Services list still
  shows `idle` and that is fine — it is a different question.
- The page issues roughly twice the queries the old one did (current + previous window for services,
  RUM vitals and hosts, plus rules and incidents). All of them are small aggregations that the other
  views already poll at 15s.
- Every threshold the board judges on is imported from the existing per-signal models
  (`serviceHealth.ts`, `hostStats.ts`, the API's own vital `rating`, the tenants API's own
  staleness bands). The board adds no fourth opinion about what "bad" means.
