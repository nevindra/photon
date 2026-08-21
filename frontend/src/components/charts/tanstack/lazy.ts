// Lazy handles for the TanStack Charts layer.
//
// It lives in its own module for two reasons. Rollup gives the dynamic `import()` its own chunk
// (~97 kB raw / ~34 kB gzipped — see `docs/frontend.md`), so nobody opening `/logs` pays for a
// chart engine they will not see. And a component that renders through it can be tested by stubbing
// this one named export, instead of stubbing a module Vue's async-component resolution probes for
// internal flags.
import { defineAsyncComponent } from 'vue'

export const LazyStackedBandChart = defineAsyncComponent(
  () => import('./StackedBandChart.vue'),
)
