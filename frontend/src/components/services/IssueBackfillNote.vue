<script setup lang="ts">
// The cut-over notice for Backend Issues.
//
// Fingerprinting happens at INGEST, not at query time (see `photon_core::span_error`), so spans
// that arrived before this feature shipped carry no `error.fingerprint` and never will. There is
// no backfill and no query-time fallback, on purpose: two grouping rules for one concept is worse
// than one honest gap in the history.
//
// This component exists so that gap is stated on the page rather than discovered. A user who
// widens the window to 30 days and sees the list thin out deserves to know why — otherwise the
// obvious reading is "Photon lost my errors".
</script>

<template>
  <p data-testid="issue-backfill-note" class="pb-1 text-[11px] leading-relaxed text-muted-foreground">
    Issues are grouped from a fingerprint written when the span is ingested. Spans received before
    this feature shipped carry none, so they appear in Traces but not here. There is no backfill:
    a second, query-time grouping rule would make "the same error" mean two different things.
  </p>
</template>
