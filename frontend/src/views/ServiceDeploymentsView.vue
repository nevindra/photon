<script setup lang="ts">
// `/services/:service/deployments` — what shipped, and what it did.
//
// Nobody sends Photon a deployment. But a service that follows OTel semconv stamps
// `service.version` on its resource, and the first time a version appears that has never appeared
// before, something was deployed. That inference is the whole feature, which is why a service that
// never sets `service.version` has an empty tab here and there is nothing to configure to fix it.
//
// The before/after diff on each row is computed HERE, from the same RED buckets the Overview tab
// draws, not by the server. Two reasons: the numbers on the row then provably agree with the chart
// above them, and the comparison window is a UI choice (how much either side of the release to
// measure) rather than a storage one.
import { computed, ref } from 'vue'
import ServiceScaffold from '@/components/services/ServiceScaffold.vue'
import DeploymentTimeline from '@/components/services/DeploymentTimeline.vue'
import ChartPanel from '@/components/charts/ChartPanel.vue'
import MetricChart from '@/components/metrics/MetricChart.vue'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Spinner } from '@/components/ui/spinner'
import { formatNumber } from '@/lib/core/format'
import { startNs, endNs, startMs, endMs, timeRange, customRange } from '@/lib/core/context'
import { useDeployments, useServiceTimeseries } from '@/lib/services/servicesQueries'
import { useServiceParam } from '@/lib/services/useServiceParam'
import { deployMarkers } from '@/lib/services/deployMarkers'

const service = useServiceParam()

// How far either side of a release the diff measures. A short window catches a crash on rollout;
// a long one catches a leak that takes an hour to show. Neither is right for both, so it is a
// control rather than a constant.
const COMPARE_CHOICES = [5, 15, 30, 60] as const
const compareMinutes = ref<number>(30)

const deploysQuery = useDeployments(service, startNs, endNs)
const deployments = computed(() => deploysQuery.data.value?.deployments ?? [])

// 96 buckets rather than the Overview's 48: the diff slices these by time, and a coarse bucket
// makes the two sides of a short comparison window the same bucket.
const seriesQuery = useServiceTimeseries(service, startNs, endNs, 96)
const buckets = computed(() => seriesQuery.data.value ?? [])

const loading = computed(() => deploysQuery.isFetching.value || seriesQuery.isFetching.value)

// A toggle group emits `undefined` when the active item is clicked again. There is no "no
// comparison window" state, so that deselect is ignored rather than turned into NaN.
function setCompare(v: unknown): void {
  const n = Number(v)
  if (Number.isFinite(n) && n > 0) compareMinutes.value = n
}

const markers = computed(() => deployMarkers(deployments.value))

// `MetricChart` takes `t` in nanoseconds; these buckets are epoch ms.
const MS_TO_NS = 1e6
const errorSeries = computed(() =>
  !buckets.value.length
    ? []
    : [
        {
          labels: { series: 'error rate' },
          points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: (b.error_rate ?? 0) * 100 })),
          exemplars: [],
        },
      ],
)
const p99Series = computed(() =>
  !buckets.value.length
    ? []
    : [
        {
          labels: { percentile: 'p99' },
          points: buckets.value.map((b) => ({ t: b.ts * MS_TO_NS, v: b.p99 != null ? Number(b.p99) / 1e6 : null })),
          exemplars: [],
        },
      ],
)
</script>

<template>
  <ServiceScaffold :service="service" crumb-suffix="Deployments">
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-5">
      <div class="flex flex-wrap items-center gap-2.5 text-xs text-muted-foreground">
        <span class="font-mono tabular-nums text-foreground/80">
          {{ formatNumber(deployments.length) }} releases
        </span>
        <span class="text-border">·</span>
        <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
        <Spinner v-if="loading" size="sm">loading…</Spinner>
        <div class="ml-auto flex items-center gap-2">
          <span class="text-[11px] uppercase tracking-wider">Compare ±</span>
          <Segmented :model-value="String(compareMinutes)" @update:model-value="setCompare">
            <SegmentedItem v-for="m in COMPARE_CHOICES" :key="m" :value="String(m)">{{ m }}m</SegmentedItem>
          </Segmented>
        </div>
      </div>

      <!-- The two charts a release is judged on, with the releases drawn on them. Reading a
           regression off the row and then seeing the step in the line is the same fact twice,
           which is what makes the row trustworthy. -->
      <div class="grid gap-3 lg:grid-cols-2">
        <ChartPanel title="Error rate" subtitle="% of requests, with releases marked">
          <MetricChart
            :series="errorSeries"
            :markers="markers"
            unit="%"
            :y-range="[0, 100]"
            :start-ms="startMs"
            :end-ms="endMs"
            :loading="seriesQuery.isFetching.value"
          />
        </ChartPanel>
        <ChartPanel title="p99 latency" subtitle="ms, with releases marked">
          <MetricChart
            :series="p99Series"
            :markers="markers"
            unit="ms"
            :start-ms="startMs"
            :end-ms="endMs"
            :loading="seriesQuery.isFetching.value"
          />
        </ChartPanel>
      </div>

      <section class="flex flex-col gap-2">
        <span class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Releases</span>
        <div class="rounded-xl border border-border bg-card px-4">
          <DeploymentTimeline
            :deployments="deployments"
            :buckets="buckets"
            :compare-minutes="compareMinutes"
          />
        </div>
        <p class="text-[11px] text-muted-foreground">
          A release is noticed the first time this service reports a
          <code class="font-mono">service.version</code> nobody has seen before, so the timestamp is
          when traffic from the new build first arrived, not when the pipeline ran. CI can post the
          commit and author to <code class="font-mono">/api/deployments</code> to fill in the rest.
        </p>
      </section>
    </main>
  </ServiceScaffold>
</template>
