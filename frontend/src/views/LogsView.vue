<script setup>
import { ref, computed, watch, onScopeDispose } from "vue";
import { useRoute, useRouter } from "vue-router";
import { refDebounced } from "@vueuse/core";
import AppShell from "@/components/common/AppShell.vue";
import SearchBar from "@/components/common/SearchBar.vue";
import LogsFilters from "@/components/logs/LogsFilters.vue";
import VolumeHistogram from "@/components/logs/VolumeHistogram.vue";
import ColumnPicker from "@/components/common/ColumnPicker.vue";
import LogTable from "@/components/logs/LogTable.vue";
import LogDetailDrawer from "@/components/logs/LogDetailDrawer.vue";
import LiveControl from "@/components/common/LiveControl.vue";
import { Switch } from "@/components/ui/switch";
import { useLiveTail, mergeLiveRows } from "@/lib/core/useLiveTail";
import { api } from "@/lib/core/api";
import { formatNumber } from "@/lib/core/format";
import { useUrlState } from "@/lib/core/useUrlState";
import {
    useServices,
    useFields,
    useSearchLogs,
    useFacet,
} from "@/lib/logs/logsQueries";
import {
    toggleFieldValue,
    toggleFieldValueNegated,
    toggleFacetValue,
    onlyFieldValue,
    removeFieldAll,
} from "@/lib/core/queryLang";
import { nextIndex } from "@/lib/core/listNav";
import {
    timeRange,
    customRange,
    nowTick,
    startNs,
    endNs,
    startMs,
    endMs,
    setCustomRange,
} from "@/lib/core/context";
import { correlate } from "@/lib/core/useCorrelate";

const route = useRoute();
const router = useRouter();

// Correlation exit: the log-detail drawer's "View trace" pivots to the waterfall route. A `?t=`
// timestamp hint (nanoseconds) narrows candidate-file selection when present. Routed through
// correlate() so the active time window + scope ride along to the trace view.
function onViewTrace({ traceId, timeHintNs }) {
    router.push(correlate({ path: "/traces/" + traceId, query: timeHintNs ? { t: timeHintNs } : {} }));
}

// --- state ---
// Time (timeRange/customRange/startNs/endNs/…) is now owned globally by lib/context.js (imported
// above) and surfaced by the ContextBar mounted in AppShell — this view no longer keeps its own
// RANGE_MS table or window refs.
// The query string is the single source of truth for service/severity filters: LogsFilters
// (pinned sections + fields catalog) derives its checked state from the `query`/`:query="debouncedQuery"`
// prop (single-state model — `facetChecked`, see facet-single-state-model.md) and its clicks
// rewrite `text`. So the search bar's pills and the panel can never desync (with the same ~180ms
// debounce lag the main row search already has — see `debouncedQuery`, below).
const text = ref("");
// The attribute columns the user has toggled on in the column picker.
const columns = ref([]);
// Adapt the field catalog to the shared ColumnPicker's shape ([{ key, label }]). LogTable's
// Time/Level/Service/Message columns are hardcoded and NOT gated by the `columns` prop — that
// prop only ever renders `row.attributes?.[key]` — so `kind: 'fixed'` (timestamp, severity_text,
// body, trace_id, span_id, scope_name, …) and `kind: 'promoted'` (service.name, already shown by
// the hardcoded Service column) fields would add a permanently-blank column if offered here.
// Only `attribute` fields are actually addable, so that's the only kind presented.
const columnFields = computed(() =>
    fields.value
        .filter((f) => f.kind === "attribute")
        .map((f) => ({
            key: f.name,
            label: f.name,
        })),
);
const selectedColumns = computed(() => new Set(columns.value));
function toggleColumn(key) {
    columns.value = columns.value.includes(key)
        ? columns.value.filter((c) => c !== key)
        : [...columns.value, key];
}
const queryError = ref(null);
const selectedId = ref(null);
const drawerOpen = ref(false);
const pollMs = ref(false); // drives searchQuery's refetchInterval: false (manual/live) or a poll cadence (ms)
const showChart = ref(true);

// `nowTick` (imported from context) is re-anchored to Date.now() at each search (in buildRequest,
// below) so the now-anchored window (and therefore the histogram / facet rail / field catalog)
// tracks real time instead of caching the timestamp of the first render. It's a shared module
// singleton — advancing it here also keeps every other context-window consumer fresh.
const toNs = (ms) => (BigInt(Math.round(ms)) * 1_000_000n).toString();

// Rounded aggregate window for the pinned Services/Severity facet counts, the fields catalog, and the
// volume histogram (mirrors TracesExplorer's aggStartMs/aggEndMs). Those queries key off the
// window, so feeding them the raw now-anchored startMs/endMs would churn their cache keys on
// every idle 12s nowTick advance — a re-fetch for a sub-second window shift nobody can see, which
// also drops `data` to `undefined` mid-flight and flashes the counts/chart to 0. Snapping to a 60s
// bucket (floor the start, ceil the end) holds the keys steady between real minute boundaries. The
// main log SEARCH keeps the PRECISE window via `buildRequest` — this only feeds the aggregates.
const AGG_BUCKET_MS = 60_000;
const aggStartMs = computed(
    () => Math.floor(startMs.value / AGG_BUCKET_MS) * AGG_BUCKET_MS,
);
const aggEndMs = computed(
    () => Math.ceil(endMs.value / AGG_BUCKET_MS) * AGG_BUCKET_MS,
);
const aggStartNs = computed(() => toNs(aggStartMs.value));
const aggEndNs = computed(() => toNs(aggEndMs.value));

// --- URL persistence (seeds refs from the query string, then keeps it in sync) ---
// Only `text` is persisted here: the `range`/`from`/`to` keys are now owned globally by
// lib/context.js's own URL sync (seeded/started once in main.js), not per-view. Seeded BEFORE the
// debounce/query wiring so the first (mount) fetch already carries the seeded query.
useUrlState({ text });

// Correlation entry: a span/trace → logs pivot lands here as `/logs?q=<query>` (router.push), so a
// FRESH LogsView mounts with the query already in the route. Seed `text` from `route.query.q`
// synchronously — before the debounced ref + search query are created below — so the initial fetch
// carries the pivot query. This is the router-native counterpart to useUrlState's window.location
// seed above (equal in a browser, but authoritative under memory-history / deep links); a plain
// (null-q) mount leaves `text` as useUrlState set it.
if (typeof route.query.q === "string" && route.query.q) text.value = route.query.q;

// Debounced query text feeds the search KEY (replaces the old 180ms setTimeout). Created AFTER the
// seeds above so its initial value is the seeded query — no extra churn on mount.
const debouncedText = refDebounced(text, 180);
// Shared trimmed-debounced query for every DERIVED aggregation request (volume histogram, pinned
// Services/Severity facet counts, and the fields-catalog facet fan-out in LogsFilters). Only the
// SearchBar itself (`text`, via the toolbar's v-model) should reflect raw per-keystroke input —
// everything that turns the query into a backend aggregation request must key off this instead, or
// it fires one request per keystroke (one histogram + one per open/pinned facet field) instead of
// collapsing to a single request like the main row search already does below.
const debouncedQuery = computed(() => debouncedText.value.trim());

// Live tail: resolves the mode picker (Manual / 5s / 30s / Live) to either an SSE stream (Live) or
// a poll cadence fed into searchQuery's `refetchInterval` below via `pollMs`. Constructed AFTER
// `debouncedText` — useLiveTail's internal watch reads `query` eagerly at setup, so `debouncedText`
// must already be initialized. The `searchQuery.refetch()` reference inside `onPoll` is a forward
// reference to a `const` declared further down; that's safe because `onPoll` is only ever invoked
// later (in response to a user action), by which point `searchQuery` is long since initialized.
const liveTail = useLiveTail({
    grain: "logs",
    query: computed(() => debouncedText.value.trim()),
    onPoll: (v) => {
        if (v === "once") {
            searchQuery.refetch();
            return;
        }
        pollMs.value = v;
    },
});

// --- services + field catalog (TanStack Query) ---
const servicesQuery = useServices();
const servicesList = computed(() => servicesQuery.data.value ?? []);
// Field catalog (drives the facet rail + column picker). Keyed off the ROUNDED aggregate window
// (aggStartNs/aggEndNs), NOT the precise startNs/endNs, so it (a) dedupes with LogsFilters' own
// useFields call — same 60s-bucket key means one shared request, not two — and (b) doesn't churn
// (refetch + flash the ColumnPicker's list empty) on every idle 12s nowTick advance. Same 60s-bucket
// rationale as the aggregate window above. The main row SEARCH still uses the precise window.
const fieldsQuery = useFields(aggStartNs, aggEndNs);
const fields = computed(() => fieldsQuery.data.value ?? []);

// --- row search (TanStack Query) ---
// Key on the RELATIVE descriptor only (query text, range, custom range, limit) — the absolute
// now-anchored window is resolved at fetch time in buildRequest, so the key never churns per-ms.
const searchKey = computed(() => ({
    query: debouncedText.value.trim(),
    timeRange: timeRange.value,
    customRange: customRange.value,
    limit: 500,
}));
function buildRequest() {
    // Resolve the now-anchored window at FETCH time so every (re)fetch — including each live-tail
    // refetchInterval poll — advances "now". Re-anchoring the shared context `nowTick` here keeps
    // the histogram / facet rail / field catalog windows (and any other context consumer) aligned
    // with the same instant as the results. `startNs`/`endNs` (imported from context) are computed
    // refs, so reading them right after the bump reflects the freshly re-anchored window.
    if (!customRange.value) nowTick.value = Date.now();
    return {
        start_ts_nanos: startNs.value,
        end_ts_nanos: endNs.value,
        // Service/severity filtering now lives entirely in the `query` grammar (`service:`/`level:`),
        // so the structured lists stay empty — sending the derived values too would double-filter.
        // Keys are kept so the backend request contract is unchanged. This also lets negated
        // `-service:x` terms work, which the structured path couldn't express.
        services: [],
        severities: [],
        // Use the SAME debounced text the key is derived from, so the request and its cache key match.
        query: debouncedText.value.trim(),
        limit: 500,
    };
}
const searchQuery = useSearchLogs(searchKey, buildRequest, {
    // Poll only when the mode picker resolved to a numeric cadence (5s/30s) — Live mode streams via
    // `liveTail` instead (see `displayRows` below) and Manual never polls. Still paused for a
    // pinned custom range or while the drawer is open, same as before.
    refetchInterval: computed(() =>
        typeof pollMs.value === "number" && !customRange.value && !drawerOpen.value
            ? pollMs.value
            : false,
    ),
});

// `api.search` returns an envelope: the loaded row page plus the full-match-set total and query
// time. `rows` holds the page; `matchedCount` is a true COUNT(*) (not rows.length) and `elapsedMs`
// is the query time — both feed the toolbar. `loading` mirrors the in-flight fetch state.
const rows = computed(() => searchQuery.data.value?.rows ?? []);
const matchedCount = computed(() => searchQuery.data.value?.matched_count ?? 0);
const elapsedMs = computed(() => searchQuery.data.value?.elapsed_ms ?? 0);
const loading = computed(() => searchQuery.isFetching.value);

// What the table actually renders: in Live mode the live-streamed rows (prepended by the SSE stream
// via `liveTail`) MERGED on top of the current search page as a frozen baseline — so entering Live
// keeps the already-loaded rows visible instead of blanking to the empty stream buffer; the search
// page otherwise (manual / 5s / 30s poll). Selection lookups below key off THIS (not `rows`) so
// clicking a live-streamed row still resolves to a real record for the detail drawer.
const displayRows = computed(() =>
    liveTail.mode.value === "live"
        ? mergeLiveRows(liveTail.rows.value, rows.value)
        : rows.value,
);

// 400 contract: when api.search throws status 400, publish a FRESH { message, offset } object each
// time an error is recorded (SearchBar's error-suppression watch keys off the prop REFERENCE
// changing, not a deep-equal), and clear it on each successful fetch. Non-400s never reach here —
// api.search mock-falls-back for them without throwing. `errorUpdatedAt`/`dataUpdatedAt` tick on
// every resolve, guaranteeing a new reference per 400 even for an identical repeated error.
watch(
    [
        () => searchQuery.errorUpdatedAt.value,
        () => searchQuery.dataUpdatedAt.value,
    ],
    () => {
        const e = searchQuery.error.value;
        if (searchQuery.isError.value && e?.status === 400) {
            queryError.value = {
                message: e.body?.error ?? "invalid query",
                offset: e.body?.offset ?? null,
            };
        } else if (searchQuery.isSuccess.value) {
            queryError.value = null;
        }
    },
);

// Drop the selection when the displayed rows no longer contain it (a fresh search page, OR the
// live-tail buffer aging the selected row out past its cap).
watch(displayRows, (rs) => {
    if (selectedId.value != null && !rs.some((r) => r.id === selectedId.value))
        selectedId.value = null;
});

// --- pinned Services/Severity counts (LogsFilters) ---
// A genuine facet fetch, NOT a tally over `rows.value`: the loaded row page is already filtered
// by every active term (including the field's own), so a currently-excluded (or, in include-mode,
// simply not-included) value would always tally to 0 there — dishonest under the single-state
// model, where an unchecked value must still show what re-checking it would surface.
// `removeFieldAll` strips only the field's OWN terms (both `field:` includes and `-field:`
// excludes) so each section counts against every OTHER active filter, mirroring
// TracesFilters' per-section strip (LogsFilters' pinned sections stay prop-driven — so
// the fetch lives here instead of inside the panel). `level` itself isn't facetable (a severity
// bucket, not a stored column — see resolver.rs's `resolve_field_name`), so the Severity facet
// queries the raw `severity_text` column and folds to the lower-case keys `SEVERITIES` uses.
const serviceCountQuery = computed(() =>
    removeFieldAll(debouncedQuery.value, "service"),
);
const severityCountQuery = computed(() =>
    removeFieldAll(debouncedQuery.value, "level"),
);
const serviceFacetQuery = useFacet(
    "service.name",
    serviceCountQuery,
    aggStartNs,
    aggEndNs,
);
const severityFacetQuery = useFacet(
    "severity_text",
    severityCountQuery,
    aggStartNs,
    aggEndNs,
);
const serviceCounts = computed(() => {
    const m = {};
    for (const v of serviceFacetQuery.data.value?.values ?? []) m[v.value] = v.count;
    return m;
});
const severityCounts = computed(() => {
    const m = {};
    for (const v of severityFacetQuery.data.value?.values ?? [])
        m[String(v.value).toLowerCase()] = v.count;
    return m;
});

// --- derived views over the loaded rows ---
const errorCount = computed(
    () =>
        rows.value.filter(
            (r) => r.severity === "error" || r.severity === "fatal",
        ).length,
);
const selectedRow = computed(
    () => displayRows.value.find((r) => r.id === selectedId.value) ?? null,
);
// Position of the open row within the visible list — feeds the drawer's prev/next nav counter.
const selectedIndex = computed(() =>
    displayRows.value.findIndex((r) => r.id === selectedId.value),
);

// --- handlers ---
// Rail clicks edit `text`; LogsFilters derives its checked state from the `query`
// prop, so the search bar's pills and the panel can never desync. LogTable's per-row "filter to
// this severity" quick action (below, `@filter-severity`) still uses plain inclusion
// (`toggleFieldValue`) — a distinct, single-purpose action, not the rails' single-state toggle —
// so it's kept separate from the unified facet handlers below.
function toggleSeverity(key) {
    text.value = toggleFieldValue(text.value, "level", key);
}
// Histogram drag-zoom pins a custom range via the shared context action (the preset time picker
// itself now lives in ContextBar, mounted globally in AppShell — this view no longer owns an
// `onRange` handler for it).
function onZoom({ startMs: s, endMs: e }) {
    setCustomRange({ startMs: s, endMs: e });
    // A pinned custom range is incompatible with a now-anchored live stream — drop back to Manual
    // (matches the pre-existing "pause live for custom range" behavior for poll modes).
    if (liveTail.mode.value === "live") liveTail.setMode("manual");
}
// The three unified single-state facet emits, shared by LogsFilters' pinned sections + catalog (see
// facet-single-state-model.md's "Parent contract"): row click toggles one value's checked state
// (mode-aware — excludes in all-mode, includes in include-mode; see `toggleFacetValue`); hover
// "Only" narrows the field to exactly one value, clearing its other includes/excludes first
// (`onlyFieldValue`); Clear All resets the field to its default all-checked state, dropping both
// signs of its terms (`removeFieldAll`). The `text` watch re-runs the search, so checkboxes,
// search-bar pills, and results can never desync.
function onToggleValue({ field, value }) {
    text.value = toggleFacetValue(text.value, field, value);
}
function onOnlyValue({ field, value }) {
    text.value = onlyFieldValue(text.value, field, value);
}
function onClearField(field) {
    text.value = removeFieldAll(text.value, field);
}
function onSelect(id) {
    selectedId.value = id;
    drawerOpen.value = true;
}

// Drawer prev/next while open: step ±1 through the visible rows (clamped at the ends by
// `nextIndex`) and re-select — the same index math LogTable.moveSelection uses, so the table's
// `watch(selectedId)` scrolls the newly-selected row into view.
function stepSelection(delta) {
    const i = nextIndex(displayRows.value.length, selectedIndex.value, delta);
    const row = displayRows.value[i];
    if (row) onSelect(row.id);
}

// Per-field filter-in / filter-out from the drawer. Reuses the same query-grammar writers the
// rail/facets use, so the search bar's pills stay the single source of truth.
function onFilterValue({ field, value, negate }) {
    text.value = negate
        ? toggleFieldValueNegated(text.value, field, value)
        : toggleFieldValue(text.value, field, value);
}

// Opening the drawer pauses live-tail prepend (streamed rows keep arriving into the pending
// buffer, surfaced via the "N new" pill) so the inspected row doesn't get yanked around; closing
// it resumes.
watch(drawerOpen, (o) => liveTail.setPaused(o));

// Pause-on-scroll: scrolling the table off row 0 pauses prepend the same way the drawer does;
// scrolling back to the top resumes it. Harmless outside Live mode (paused has no visible effect
// when `displayRows` isn't reading `liveTail.rows`).
function onTableScroll(atTop) {
    liveTail.setPaused(!atTop);
}

// --- aggregates slow-refresh (histogram / facet rail / field catalog) ---
// Those three derive their window from `nowTick` (see `endMs`/`startMs` above), which normally
// only advances when the MAIN search fetches (`buildRequest`). In Live mode the main search no
// longer polls (the SSE stream feeds the table instead — see `displayRows`), so without this timer
// the aggregates would freeze at the instant Live was selected. A de-emphasized 12s background
// tick keeps them roughly fresh without hammering the facet/histogram endpoints on every SSE
// flush. Manual mode relies solely on the ⟳ refresh button; a pinned custom range never
// auto-advances (mirrors the main search's own gating).
let aggregatesTimer = null;
function stopAggregatesTimer() {
    if (aggregatesTimer) {
        clearInterval(aggregatesTimer);
        aggregatesTimer = null;
    }
}
watch(
    () => [liveTail.mode.value, customRange.value],
    ([mode, cr]) => {
        stopAggregatesTimer();
        if (mode !== "manual" && !cr) {
            aggregatesTimer = setInterval(() => {
                nowTick.value = Date.now();
            }, 12000);
        }
    },
    { immediate: true },
);
onScopeDispose(stopAggregatesTimer);

// A 1s ticking clock purely for the "as of Xs ago" caption's live seconds count — it does NOT
// re-anchor the aggregates window itself (that's `nowTick`, above).
const clockTick = ref(Date.now());
const clockTimer = setInterval(() => {
    clockTick.value = Date.now();
}, 1000);
onScopeDispose(() => clearInterval(clockTimer));
const aggregatesAsOfSec = computed(() =>
    Math.max(0, Math.round((clockTick.value - nowTick.value) / 1000)),
);
</script>

<template>
    <AppShell :mock="api.mock" crumb="Logs">
        <template #toolbar>
            <SearchBar
                :model-value="text"
                @update:model-value="text = $event"
                :services="servicesList"
                :error="queryError"
            />
        </template>

        <div class="flex flex-1 min-h-0">
            <aside
                class="flex w-[210px] flex-none flex-col overflow-y-auto border-r border-border"
            >
                <LogsFilters
                    :services="servicesList"
                    :query="debouncedQuery"
                    :service-counts="serviceCounts"
                    :severity-counts="severityCounts"
                    :start-ms="aggStartMs"
                    :end-ms="aggEndMs"
                    @toggle-value="onToggleValue"
                    @only-value="onOnlyValue"
                    @clear-field="onClearField"
                />
            </aside>

            <main class="flex flex-col flex-1 min-w-0 min-h-0">
                <div class="px-5 pb-3 pt-5">
                    <div class="mb-4 flex items-center gap-2.5 pb-3">
                        <span class="text-xs font-medium text-foreground"
                            >Frequency chart</span
                        >
                        <Switch
                            id="chart-toggle"
                            v-model="showChart"
                            aria-label="Toggle frequency chart"
                        />
                        <span
                            v-if="liveTail.mode.value !== 'manual'"
                            class="font-mono text-[10px] text-muted-foreground/60"
                        >
                            as of {{ aggregatesAsOfSec }}s ago
                        </span>
                        <span
                            class="ml-auto font-mono text-[11px] text-muted-foreground"
                        >
                            {{
                                customRange
                                    ? "custom range"
                                    : `last ${timeRange}`
                            }}
                        </span>
                    </div>
                    <VolumeHistogram
                        v-if="showChart"
                        :query="debouncedQuery"
                        :start-ms="aggStartMs"
                        :end-ms="aggEndMs"
                        @zoom="onZoom"
                    />
                </div>

                <div
                    class="flex items-center gap-2.5 px-5 pb-2 text-xs text-muted-foreground"
                >
                    <span class="font-mono tabular-nums text-foreground/80">
                        {{ formatNumber(matchedCount) }} lines
                    </span>
                    <span class="text-border">·</span>
                    <span class="font-mono tabular-nums"
                        >{{ elapsedMs }} ms</span
                    >
                    <span class="text-border">·</span>
                    <span class="font-mono">{{
                        customRange ? "custom range" : `last ${timeRange}`
                    }}</span>
                    <template v-if="errorCount > 0">
                        <span class="text-border">·</span>
                        <span class="font-mono text-sev-error"
                            >{{ formatNumber(errorCount) }} errors</span
                        >
                    </template>
                    <span
                        v-if="loading"
                        class="font-mono text-muted-foreground/70"
                        >searching…</span
                    >

                    <div class="ml-auto flex items-center gap-2">
                        <ColumnPicker
                            :available="columnFields"
                            :selected="selectedColumns"
                            @toggle="toggleColumn"
                        />
                        <LiveControl
                            :mode="liveTail.mode.value"
                            :status="liveTail.status.value"
                            :rate="liveTail.rate.value"
                            @update:mode="liveTail.setMode"
                            @refresh="liveTail.refresh"
                        />
                    </div>
                </div>

                <button
                    v-if="liveTail.newCount.value > 0"
                    type="button"
                    class="flex w-full items-center justify-center gap-1.5 border-b border-border bg-muted/60 py-1 font-mono text-[11px] text-foreground transition-colors hover:bg-muted"
                    @click="liveTail.jumpToLatest()"
                >
                    ↑ {{ formatNumber(liveTail.newCount.value) }} new lines — jump to latest
                </button>

                <LogTable
                    :rows="displayRows"
                    :columns="columns"
                    :selected-id="selectedId"
                    :loading="loading"
                    @select="onSelect"
                    @filter-severity="toggleSeverity"
                    @scroll-top-change="onTableScroll"
                />
            </main>
        </div>

        <LogDetailDrawer
            :row="selectedRow"
            v-model:open="drawerOpen"
            :index="selectedIndex"
            :total="displayRows.length"
            @view-trace="onViewTrace"
            @prev="stepSelection(-1)"
            @next="stepSelection(1)"
            @filter-value="onFilterValue"
        />
    </AppShell>
</template>
