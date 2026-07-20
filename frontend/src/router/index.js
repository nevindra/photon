import { createRouter, createWebHistory } from "vue-router";
import { authed, needsSetup, hydrate } from "@/lib/core/auth";
import { syncContextToUrl } from "@/lib/core/context";
import LoginView from "@/views/LoginView.vue";
import OnboardingView from "@/views/OnboardingView.vue";
import HomeView from "@/views/HomeView.vue";
import LogsView from "@/views/LogsView.vue";
import TracesExplorer from "@/views/TracesExplorer.vue";
import TraceDetailView from "@/views/TraceDetailView.vue";
import ServicesListView from "@/views/ServicesListView.vue";
import ServiceDetailView from "@/views/ServiceDetailView.vue";
import MetricsExplorer from "@/views/MetricsExplorer.vue";
import RumAppsView from "@/views/RumAppsView.vue";
import RumVitalsView from "@/views/RumVitalsView.vue";
import RumPagesView from "@/views/RumPagesView.vue";
import RumPageDetailView from "@/views/RumPageDetailView.vue";
import RumErrorsView from "@/views/RumErrorsView.vue";
import RumErrorDetailView from "@/views/RumErrorDetailView.vue";
import UptimeDashboard from "@/views/UptimeDashboard.vue";
import DataView from "@/views/DataView.vue";
import InfraHostsView from "@/views/InfraHostsView.vue";
import InfraHostDetailView from "@/views/InfraHostDetailView.vue";

const routes = [
  { path: "/", redirect: "/home" },
  { path: "/login", name: "login", component: LoginView },
  { path: "/onboarding", name: "onboarding", component: OnboardingView },
  { path: "/home", name: "home", component: HomeView },
  { path: "/logs", name: "logs", component: LogsView },
  { path: "/traces", name: "traces", component: TracesExplorer },
  // The RED sub-view used to live here as a Traces sub-view; it has been promoted to the
  // top-level Services (APM) section below. Kept as a redirect for old links/bookmarks.
  // Declared before `/traces/:traceId` so the static path wins.
  { path: "/traces/metrics", redirect: "/services" },
  {
    path: "/traces/:traceId",
    name: "trace-detail",
    component: TraceDetailView,
  },
  // Services (APM): per-service health list + per-service detail dashboard.
  { path: "/services", name: "services", component: ServicesListView },
  {
    path: "/services/:service",
    name: "service-detail",
    component: ServiceDetailView,
  },
  // Top-level OTLP Metrics Explorer (Milestone 3). Distinct from the Services section above.
  { path: "/metrics", name: "metrics", component: MetricsExplorer },
  // Catalog sub-view of the Metrics Explorer. Same component as /metrics (reused instance keeps
  // builder/time state across the sub-nav); the component derives explore-vs-catalog mode from the path.
  { path: "/metrics/catalog", name: "metrics-catalog", component: MetricsExplorer },
  // RUM (Real User Monitoring): app-scoped sub-routes. Static sub-paths (/pages, /errors) are
  // declared before the dynamic `:appId` catch-alls so they don't get swallowed by them, and
  // `/rum/:appId/pages/:route` before the bare `/rum/:appId` for the same reason.
  { path: "/rum", name: "rum", component: RumAppsView },
  { path: "/rum/:appId", name: "rum-app", component: RumVitalsView },
  { path: "/rum/:appId/pages", name: "rum-pages", component: RumPagesView },
  { path: "/rum/:appId/pages/:route", name: "rum-page-detail", component: RumPageDetailView },
  { path: "/rum/:appId/errors", name: "rum-errors", component: RumErrorsView },
  { path: "/rum/:appId/errors/:fingerprint", name: "rum-error-detail", component: RumErrorDetailView },
  { path: "/uptime", name: "uptime", component: UptimeDashboard },
  // Infrastructure (host/GPU resource monitoring): host list + per-host detail. Static `/infra`
  // before the dynamic `/infra/:host` for the same reason as the RUM sub-routes above.
  { path: "/infra", name: "infra", component: InfraHostsView },
  { path: "/infra/:host", name: "infra-host", component: InfraHostDetailView },
  { path: "/data", name: "data", component: DataView },
  // Alerts (webhook-alert engine, Manage group): lazy-loaded since it's not on the critical
  // first-paint path and its own subtree (rule builder, condition builder, channel dialogs) is
  // sizeable — mirrors the split-chunk treatment other heavier views get from Vite's default
  // route-based code splitting once they're behind a dynamic import.
  {
    path: "/alerts",
    name: "alerts",
    component: () => import("../views/AlertsView.vue"),
  },
  { path: "/:pathMatch(.*)*", redirect: "/home" },
];

export const router = createRouter({ history: createWebHistory(), routes });

// Auth gate. `hydrate()` (cached) restores auth state from the server on the first navigation,
// which is what makes a page refresh keep you signed in. Onboarding takes precedence: with no
// users, everything routes to /onboarding; once a user exists, /onboarding bounces to /login.
router.beforeEach(async (to) => {
  await hydrate();
  const isPublic = to.path === "/login" || to.path === "/onboarding";
  if (needsSetup.value) {
    return to.path === "/onboarding" ? true : { path: "/onboarding" };
  }
  if (to.path === "/onboarding") {
    return { path: "/login" };
  }
  if (!isPublic && !authed.value) {
    return { path: "/login", query: { redirect: to.fullPath } };
  }
  if (to.path === "/login" && authed.value) {
    return typeof to.query.redirect === "string" ? to.query.redirect : "/logs";
  }
  return true;
});

// Context (range/from/to/scope) lives in context.ts and only re-syncs to the URL when those
// refs change (see startContextUrlSync in context.ts) — so a bare `router.push('/services')`
// (NavRail world switch, list drill-ins, back buttons) would otherwise land on a URL missing
// the active range/scope, resetting them on reload/share. Re-run the merge-write after every
// navigation via `history.replaceState` (no new history entry, no navigation → no afterEach loop).
router.afterEach(() => syncContextToUrl());
