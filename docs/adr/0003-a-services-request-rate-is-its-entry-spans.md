# A service's request rate is its entry spans, not every span it emits

Status: accepted

`red_over` aggregated over every span kind. A service that instruments generously — nested INTERNAL spans
around each stage of its own work — reported several times the throughput of a service doing identical work
with one span per request. The two were never comparable, and the Services table sorted them against each
other anyway.

The error rate had the same flaw from the other side. A CLIENT span is a call this service *makes*. When a
database call times out and the code retries successfully, the request the user saw succeeded, but the failed
CLIENT span counted as a service error. Services that handle failure well looked worse than services that let
it through.

**A service's requests are the spans where work entered it: `kind` SERVER or CONSUMER.** Rate, error rate,
duration percentiles, and the Apdex bands are all computed over that set. Datadog and SigNoz both draw the
same line.

## The second number stays

Total span count is not deleted, it is separated: `span_count` sits beside `count`, shown as a sub-line under
Traffic. Both numbers are real and they answer different questions — "how many requests did you serve" and
"how much work did you record". Collapsing them into one was the original mistake; picking one and discarding
the other would repeat it in the opposite direction.

## Services with no entry spans

A worker that only emits INTERNAL spans, or an SDK that never sets `kind`, would vanish from the Services
list under a strict rule — precisely when it broke. So the entry set is a cascade, first non-empty wins:

1. `kind IN (SERVER, CONSUMER)`
2. spans with no parent (trace roots)
3. every span

The rule that produced the number travels with it as `basis`, and the third case earns a visible
`span-based` badge on the row. The alternative — a strictly correct definition ("a span whose parent belongs
to a different service") — needs a self-join on every span in the window to learn each parent's service, and
that is far too expensive for the read path that every Backend page depends on.

A service never disappears, and a number never quietly changes meaning.

## Consequences

- Every rate and error rate on the Backend pages changes on upgrade, in most cases downward. This is a
  correction, not a regression, and the release notes must say so plainly.
- Apdex bands now sum to `count`, not to `span_count`.
- Alert rules written against the old inflated rate will need their thresholds revisited. There is no
  migration for this; it needs a release note.
- `status_code == 2` remains the definition of `error_count`. Task 2 of the revamp plan adds `http_4xx` and
  `http_5xx` counters **beside** it rather than redefining it, so "error rate" keeps meaning one thing.
