<script setup>
import PhotonMark from '@/components/common/PhotonMark.vue'

defineProps({
  // Small line under the wordmark (e.g. the page's purpose).
  subtitle: { type: String, default: '' },
})
</script>

<template>
  <main class="auth-main">
    <!-- Branded backdrop: a soft cyan glow + a faint grid masked to the centre.
         Pure CSS, token-driven, static (no motion) — sets the tone without noise. -->
    <div class="auth-backdrop" aria-hidden="true">
      <div class="auth-glow" />
      <div class="auth-grid" />
    </div>

    <div
      class="relative w-full max-w-[400px] rounded-lg border border-border bg-surface-1 p-8 shadow-2"
    >
      <div class="flex items-center gap-3">
        <div class="relative">
          <PhotonMark :size="30" />
          <span
            class="absolute -bottom-0.5 -right-0.5 size-2 rounded-full bg-brand ring-2 ring-surface-1"
            aria-hidden="true"
          />
        </div>
        <span class="text-2xl font-semibold tracking-tight text-foreground">photon</span>
      </div>
      <p v-if="subtitle" class="mt-2.5 text-sm text-muted-foreground">{{ subtitle }}</p>

      <slot />
    </div>
  </main>
</template>

<style scoped>
.auth-main {
  position: relative;
  display: flex;
  height: 100%;
  min-height: 0;
  flex: 1 1 0%;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  padding: 1.5rem;
  background: hsl(var(--background));
}
.auth-backdrop {
  position: absolute;
  inset: 0;
  pointer-events: none;
}
.auth-glow {
  position: absolute;
  left: 50%;
  top: 44%;
  height: 560px;
  width: 560px;
  transform: translate(-50%, -50%);
  border-radius: 9999px;
  background: hsl(var(--brand) / 0.14);
  filter: blur(90px);
}
.auth-grid {
  position: absolute;
  inset: 0;
  opacity: 0.5;
  background-image:
    linear-gradient(hsl(var(--border)) 1px, transparent 1px),
    linear-gradient(90deg, hsl(var(--border)) 1px, transparent 1px);
  background-size: 40px 40px;
  -webkit-mask-image: radial-gradient(ellipse 55% 50% at 50% 45%, #000 12%, transparent 72%);
  mask-image: radial-gradient(ellipse 55% 50% at 50% 45%, #000 12%, transparent 72%);
}
</style>
