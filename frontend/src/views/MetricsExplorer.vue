<script setup>
// The OTLP Metrics Explorer shell (Milestone 3, Phase 3). Mirrors TracesExplorer.vue's skeleton
// (time/now-anchor/URL-state machinery copied verbatim) and composes the pure metrics components:
// a query builder row → a two-column [chart + legend | metadata] grid, plus a browse-all Catalog
// tab. The view owns ALL server state (via lib/metricsQueries.js) and the not-chartable /
// filter-400 handling; the child components stay pure (props-in).
import { ref, computed, watch } from "vue";
import { useRouter, useRoute } from "vue-router";
import { useQuery } from "@tanstack/vue-query";
import { refDebounced } from "@vueuse/core";
import AppShell from "@/components/common/AppShell.vue";
import { Card } from "@/components/ui/card";
import { NavTabs, NavTabItem } from "@/components/ui/nav-tabs";
import MetricQueryRow from "@/components/metrics/MetricQueryRow.vue";
import MetricChart from "@/components/metrics/MetricChart.vue";
import MetricLegendTable from "@/components/metrics/MetricLegendTable.vue";
import MetricMetaPanel from "@/components/metrics/MetricMetaPanel.vue";
import MetricCatalog from "@/components/metrics/MetricCatalog.vue";
import MetricQuickStarts from "@/components/metrics/MetricQuickStarts.vue";
import { BarChart3, AlertTriangle } from "lucide-vue-next";
import { api } from "@/lib/core/api";
import { formatNumber } from "@/lib/core/format";
import { useLiveTail } from "@/lib/core/useLiveTail";
import {
    useMetricCatalog,
    useMetricMetadata,
    useMetricSeries,
} from "@/lib/metrics/metricsQueries";
import { isChartable, defaultAggForType } from "@/lib/metrics/metricFields";
import { createMetricFavorites } from "@/lib/metrics/metricFavorites";
import { parseViz, serializeViz, availableViz } from "@/lib/metrics/metricViz";
// App-wide time window (Task 9): the global context owns timeRange/customRange + the derived
// window. ContextBar (in AppShell) provides the picker; this view just reads the window.
import {
    timeRange,
    customRange,
    startNs,
    endNs,
    startMs,
    endMs,
    setCustomRange,
} from "@/lib/core/context";
import { correlate } from "@/lib/core/useCorrelate";

const router = useRouter();
const route = useRoute();

// --- state ---
// Explore-vs-catalog mode is derived from the route path (both /metrics and /metrics/catalog map
// to this same component, so Vue Router reuses the instance and in-memory builder/time state
// persists across the sub-nav).
const mode = computed(() =>
    route.path === "/metrics/catalog" ? "catalog" : "explore",
);

// Builder state.
const metric = ref("");
const agg = ref(null); // null = "auto" (server smart-default)
const groupBy = ref([]);
const filter = ref("");
// Debounced so typing in the filter box doesn't re-key the series query on every keystroke —
// MetricQueryRow still gets the RAW `filter` (typing stays responsive); only the seriesKey/
// buildRequest read the settled value. Mirrors TracesExplorer.vue's `debouncedText`.
const debouncedFilter = refDebounced(filter, 180);
const viz = ref("line");
const yLog = ref(false);
// The y-log toggle only applies to the line family (bar/stat/table ignore it).
const showYLogToggle = computed(() =>
    ["line", "area", "stacked"].includes(viz.value),
);

// Favorites/recent (localStorage-backed) for the metric picker.
const favStore = createMetricFavorites();
const favorites = favStore.favorites;
const recent = favStore.recent;

// Live tail: Metrics is a window-refresh chart, not a stream — `streamable: false` means Live
// resolves to a fast (2s) poll (onPoll(2000)) instead of opening an EventSource, and the
// append-only affordances (prepend/buffer/pause) `useLiveTail` exposes for logs/traces stay
// unused here. `onPoll('once')` (the LiveControl refresh button) triggers a manual refetch.
const pollMs = ref(false);
const liveTail = useLiveTail({
    grain: "metrics",
    streamable: false,
    query: computed(() => debouncedFilter.value.trim()),
    onPoll: (v) => {
        if (v === "once") {
            seriesQ.refetch();
            return;
        }
        pollMs.value = v;
    },
});

// Legend↔chart cross-link + grammar-filter 400 underline.
const highlightKey = ref(null);
const filterError = ref(null);

// The time window (timeRange/customRange + the derived startMs/endMs/startNs/endNs) now lives in
// the global context (imported above); the chart's x-axis clock advances when the context's window
// re-anchors. `seriesKey` below stays RELATIVE (timeRange/customRange only, never ns).

// --- URL persistence: the global context owns the `range`/`from`/`to` keys; metric/agg/group/q
// are layered on below (context's URL sync preserves those non-context keys). ---
// Seed the builder from the URL on reload (before the layering watcher rewrites it).
if (typeof window !== "undefined") {
    const p = new URLSearchParams(window.location.search);
    if (p.get("metric")) metric.value = p.get("metric");
    if (p.get("agg")) agg.value = p.get("agg");
    if (p.get("group")) groupBy.value = [p.get("group")];
    if (p.get("q")) filter.value = p.get("q");
    if (p.get("viz")) viz.value = parseViz(p.get("viz"));
}
// Layer metric/agg/group/q onto the URL. The global context's URL sync (flush 'pre') merge-writes
// only its own range/from/to/scope keys and preserves these builder keys, so a flush:'post' watcher
// runs after it and (re)stamps metric/agg/group/q. `timeRange` is in the dep list so a range change
// re-stamps them too (mirrors the `group` layering in MetricsView.vue:174-184).
if (typeof window !== "undefined") {
    watch(
        [timeRange, metric, agg, groupBy, filter, viz],
        () => {
            const params = new URLSearchParams(window.location.search);
            const set = (k, v) => (v ? params.set(k, v) : params.delete(k));
            set("metric", metric.value);
            set("agg", agg.value);
            set("group", groupBy.value[0] ?? "");
            set("q", filter.value);
            set("viz", serializeViz(viz.value));
            window.history.replaceState(null, "", "?" + params.toString());
        },
        { flush: "post" },
    );
}

// --- catalog (feeds MetricPicker + the Catalog tab) ---
const catalogQ = useMetricCatalog(startNs, endNs);
const catalog = computed(() => catalogQ.data.value ?? []);
const selectedEntry = computed(
    () => catalog.value.find((m) => m.name === metric.value) || null,
);
const metricType = computed(() => selectedEntry.value?.type ?? "");
const isMonotonic = computed(() => selectedEntry.value?.is_monotonic ?? null);

// --- metadata (meta panel + attribute keys for group-by/filter) ---
const metadataQ = useMetricMetadata(metric, startNs, endNs);
const metadata = computed(() => metadataQ.data.value ?? null);
const attributeKeys = computed(
    () => metadata.value?.attribute_keys ?? ["service"],
);

// --- services (grammar-filter autocomplete) ---
const servicesQ = useQuery({
    queryKey: ["services"],
    queryFn: ({ signal }) => api.services({ signal }),
    staleTime: 5 * 60 * 1000,
});
const services = computed(() => servicesQ.data.value ?? []);

// --- series ---
// All five metric types chart end-to-end (Task 6): gauge/sum via their value column,
// histogram/exp_histogram/summary via the quantile series the engine now returns.
const chartable = computed(
    () => !!metric.value && isChartable(metricType.value),
);

// RELATIVE descriptor only — never the now-anchored absolute ns (those resolve inside buildRequest).
const seriesKey = computed(() =>
    [
        metric.value,
        agg.value ?? "auto",
        groupBy.value.join(","),
        debouncedFilter.value,
        timeRange.value,
        customRange.value
            ? `${customRange.value.startMs}-${customRange.value.endMs}`
            : "",
    ].join("|"),
);
function buildRequest() {
    // Resolve the absolute window from the global context at fetch time.
    return {
        queries: [
            {
                id: "a",
                metric: metric.value,
                agg: agg.value ?? undefined, // undefined → server smart default
                group_by: groupBy.value,
                filter: debouncedFilter.value,
            },
        ],
        start: startNs.value,
        end: endNs.value,
    };
}
const seriesQ = useMetricSeries(seriesKey, buildRequest, {
    enabled: chartable,
    refetchInterval: computed(() =>
        typeof pollMs.value === "number" && !customRange.value
            ? pollMs.value
            : false,
    ),
});
const result = computed(() => seriesQ.data.value?.results?.[0] ?? null);
const series = computed(() => result.value?.series ?? []);
// Auto-fallback: an incompatible viz (e.g. `stat` under multiple series) reverts to line.
watch([viz, series], () => {
    const ok = availableViz({
        type: metricType.value,
        seriesCount: series.value.length,
    });
    if (!ok.includes(viz.value)) viz.value = "line";
});
const defaultAgg = computed(
    () =>
        result.value?.default_agg ??
        defaultAggForType(metricType.value, isMonotonic.value),
);
const capped = computed(() => seriesQ.data.value?.capped ?? false);

// --- chart summary band (the reference's stat-band, ported as the chart header) ---
// Descriptive stats across every plotted point of every visible series. Deliberately
// aggregation-agnostic — it just describes what's drawn — so it stays honest for a quantile
// line (histogram p99) as much as a counter. Null (band hidden) until there's something to plot.
const overview = computed(() => {
    let peak = -Infinity;
    let low = Infinity;
    let sum = 0;
    let n = 0;
    for (const s of series.value) {
        for (const p of s.points) {
            if (p.v == null) continue;
            if (p.v > peak) peak = p.v;
            if (p.v < low) low = p.v;
            sum += p.v;
            n += 1;
        }
    }
    if (!n) return null;
    return { count: series.value.length, peak, low, avg: sum / n };
});
// Unit shown as a faint suffix on the hero value; the dimensionless OTLP unit "1" reads as no unit.
const unitSuffix = computed(() => {
    const u = metadata.value?.unit;
    return u && u !== "1" ? u : "";
});
// Round to 2 decimals, then group-format — matches the legend table's `fmt`.
const statVal = (v) => formatNumber(Math.round(v * 100) / 100);

// Filter-grammar 400 (has a byte offset) → underline the bad token. Engine 400s (no offset — e.g.
// the histogram-quantile "later phase" message) are NOT filter errors → leave it null; the
// not-chartable placeholder already covers those. Fresh object each time so SearchBar's
// error-suppression watch (keys off reference identity) re-triggers.
watch(
    () => [
        seriesQ.error.value,
        seriesQ.errorUpdatedAt.value,
        seriesQ.dataUpdatedAt.value,
    ],
    () => {
        const e = seriesQ.error.value;
        filterError.value =
            e && e.status === 400 && e.body?.offset != null
                ? {
                      message: e.body?.error ?? "invalid filter",
                      offset: e.body.offset,
                  }
                : null;
    },
);

// --- handlers ---
function onMetric(name) {
    metric.value = name;
    agg.value = null;
    groupBy.value = [];
    filter.value = "";
    favStore.recordRecent(name);
}
function onCatalogOpen(name) {
    onMetric(name);
    router.push({ path: "/metrics", query: route.query });
}
// Exemplar CTA lands in Traces filtered to this metric's service (exemplars ship later — Phase 6).
// Routed through correlate() so the active time window + scope carry into the Traces view.
function onViewExemplars() {
    router.push(correlate({ path: "/traces", query: { sort: "slowest" } }));
}
// Brush-zoom on the chart narrows the global time window to the dragged range.
function onZoom({ startMs: s, endMs: e }) {
    if (Number.isFinite(s) && Number.isFinite(e) && e > s)
        setCustomRange({ startMs: s, endMs: e });
}
// Correlate to traces at the active window/scope (correlate() carries time+scope). A timestamp-
// scoped jump can be added when the traces view accepts an at= param; for now use the window.
function onPointClick() {
    router.push(correlate({ path: "/traces", query: { sort: "slowest" } }));
}
// Quick-start card: seed the whole builder (metric/agg/group/viz) in one shot.
function onQuickStart({ metric: m, agg: a, group_by, viz: v }) {
    metric.value = m;
    agg.value = a ?? null;
    groupBy.value = group_by ?? [];
    filter.value = "";
    if (v) viz.value = parseViz(v);
    favStore.recordRecent(m);
}

// Exposed so the view test (and future E2E) can drive builder selection directly.
defineExpose({ metric, agg, groupBy, filter, mode, viz });
</script>

<template>
    <AppShell
        active="metrics"
        :mock="api.mock"
        crumb="Metrics"
        :live="mode === 'explore'"
        :live-mode="liveTail.mode.value"
        :live-status="liveTail.status.value"
        @update:live-mode="liveTail.setMode"
        @refresh="liveTail.refresh"
    >
        <!-- Explore/Catalog route sub-nav folds into the ContextBar's search region; the Live tail
             toggle (explore-only) rides the ContextBar's live control via AppShell's passthrough. -->
        <template #toolbar>
            <NavTabs class="font-mono text-xs">
                <NavTabItem
                    :to="{ path: '/metrics', query: route.query }"
                    :active="mode === 'explore'"
                    data-testid="mode-explore"
                    class="font-mono text-xs"
                >
                    Explore
                </NavTabItem>
                <NavTabItem
                    :to="{ path: '/metrics/catalog', query: route.query }"
                    :active="mode === 'catalog'"
                    data-testid="mode-catalog"
                    class="font-mono text-xs"
                >
                    Catalog
                </NavTabItem>
            </NavTabs>
        </template>

        <div class="flex min-h-0 flex-1 flex-col gap-0">
            <!-- Explore mode -->
            <div
                v-if="mode === 'explore'"
                class="flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto p-5"
            >
                <MetricQueryRow
                    :metric="metric"
                    :catalog="catalog"
                    :agg="agg"
                    :default-agg="defaultAgg"
                    :group-by="groupBy"
                    :filter="filter"
                    :filter-error="filterError"
                    :metric-type="metricType"
                    :is-monotonic="isMonotonic"
                    :attribute-keys="attributeKeys"
                    :services="services"
                    :catalog-loading="catalogQ.isLoading.value"
                    :viz="viz"
                    :series-count="series.length"
                    :favorites="favorites"
                    :recent="recent"
                    @update:metric="onMetric"
                    @update:agg="agg = $event"
                    @update:group-by="groupBy = $event"
                    @update:filter="filter = $event"
                    @update:viz="viz = $event"
                    @toggle-favorite="favStore.toggleFavorite"
                />

                <div class="grid grid-cols-1 gap-3.5 lg:grid-cols-[1fr_268px]">
                    <Card class="overflow-hidden">
                        <!-- summary band: the signature stat header, shown once there's data to describe -->
                        <div
                            v-if="chartable && overview"
                            data-testid="chart-overview"
                            class="flex flex-wrap items-stretch gap-y-3 border-b border-border px-4 py-3"
                        >
                            <div
                                class="flex flex-col justify-center gap-1 pr-6"
                            >
                                <span
                                    class="text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground"
                                    >Series</span
                                >
                                <span
                                    class="font-mono text-[13px] tabular-nums text-foreground"
                                    >{{ formatNumber(overview.count) }}</span
                                >
                            </div>
                            <div
                                class="flex flex-col justify-center gap-1 border-l border-border px-6"
                            >
                                <span
                                    class="text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground"
                                    >Peak</span
                                >
                                <span
                                    class="flex items-baseline gap-1 font-mono text-[19px] font-semibold leading-none tracking-tight tabular-nums text-foreground"
                                >
                                    {{ statVal(overview.peak)
                                    }}<span
                                        v-if="unitSuffix"
                                        class="text-[11px] font-normal text-muted-foreground"
                                        >{{ unitSuffix }}</span
                                    >
                                </span>
                            </div>
                            <div
                                class="flex flex-col justify-center gap-1 border-l border-border px-6"
                            >
                                <span
                                    class="text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground"
                                    >Avg</span
                                >
                                <span
                                    class="font-mono text-[13px] tabular-nums text-foreground"
                                    >{{ statVal(overview.avg) }}</span
                                >
                            </div>
                            <div
                                class="flex flex-col justify-center gap-1 border-l border-border px-6"
                            >
                                <span
                                    class="text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground"
                                    >Low</span
                                >
                                <span
                                    class="font-mono text-[13px] tabular-nums text-foreground"
                                    >{{ statVal(overview.low) }}</span
                                >
                            </div>
                            <div
                                class="ml-auto flex flex-col items-end justify-center gap-1 pl-6"
                            >
                                <span
                                    class="text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground"
                                    >Window</span
                                >
                                <span
                                    class="font-mono text-[12px] text-muted-foreground"
                                    >{{
                                        customRange
                                            ? "custom"
                                            : `last ${timeRange}`
                                    }}</span
                                >
                            </div>
                            <button
                                v-if="showYLogToggle"
                                type="button"
                                data-testid="ylog-toggle"
                                class="ml-3 self-center rounded-lg border px-2 py-1 text-[11px] font-mono transition-colors"
                                :class="
                                    yLog
                                        ? 'border-brand/50 bg-brand/10 text-foreground'
                                        : 'border-border text-muted-foreground hover:text-foreground'
                                "
                                @click="yLog = !yLog"
                            >
                                log y
                            </button>
                        </div>

                        <div class="p-4">
                            <div
                                v-if="!metric"
                                class="flex h-[230px] flex-col items-center justify-center gap-3 text-muted-foreground"
                            >
                                <MetricQuickStarts
                                    :catalog="catalog"
                                    @apply="onQuickStart"
                                />
                                <span class="text-[13px]"
                                    >Pick a metric, or start from a template
                                    above.</span
                                >
                            </div>
                            <div
                                v-else-if="metricType && !chartable"
                                data-testid="chart-not-chartable"
                                class="flex h-[230px] flex-col items-center justify-center gap-1.5 px-6 text-center"
                            >
                                <BarChart3
                                    class="mb-1 size-7 text-muted-foreground opacity-30"
                                    :stroke-width="1.5"
                                />
                                <div
                                    class="text-[13px] font-medium text-foreground"
                                >
                                    This metric type can't be charted.
                                </div>
                                <div class="text-[12px] text-muted-foreground">
                                    Try a different metric or aggregation.
                                </div>
                            </div>
                            <template v-else>
                                <MetricChart
                                    :series="series"
                                    :unit="metadata?.unit || ''"
                                    :start-ms="startMs"
                                    :end-ms="endMs"
                                    :highlight-key="highlightKey"
                                    :viz="viz"
                                    :y-log="yLog"
                                    :loading="
                                        seriesQ.isFetching.value ||
                                        (!!metric && !metricType)
                                    "
                                    @zoom="onZoom"
                                    @point-click="onPointClick"
                                    @highlight="highlightKey = $event"
                                />
                                <MetricLegendTable
                                    v-if="viz !== 'stat' && viz !== 'table'"
                                    :series="series"
                                    :unit="metadata?.unit || ''"
                                    :highlight-key="highlightKey"
                                    @highlight="highlightKey = $event"
                                />
                                <p
                                    v-if="capped"
                                    class="mt-3 flex items-center gap-1.5 text-[11px] text-sev-warn"
                                >
                                    <AlertTriangle class="size-3.5 shrink-0" />
                                    Results capped — narrow the filter or
                                    group-by to see every series.
                                </p>
                            </template>
                        </div>
                    </Card>

                    <MetricMetaPanel
                        :metadata="metadata"
                        :loading="metadataQ.isFetching.value"
                        @view-exemplars="onViewExemplars"
                    />
                </div>
            </div>

            <!-- Catalog mode -->
            <div v-else class="min-h-0 flex-1 overflow-y-auto p-5">
                <MetricCatalog
                    :entries="catalog"
                    :loading="catalogQ.isLoading.value"
                    @open="onCatalogOpen"
                />
            </div>
        </div>
    </AppShell>
</template>
