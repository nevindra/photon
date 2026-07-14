<script setup>
import { TooltipProvider } from '@/components/ui/tooltip'
import { Toaster } from '@/components/ui/toast'
import { initTheme } from '@/lib/core/theme'

initTheme()
</script>

<template>
  <!-- ONE TooltipProvider ancestor for the whole app (NavRail/VolumeHistogram use Reka Tooltip). -->
  <TooltipProvider :delay-duration="200">
    <router-view />
    <!-- Mounted once, app-wide: renders every toast raised via `toast()` from anywhere
         (mutation composables, components), see components/ui/toast/useToast.ts. -->
    <Toaster />
  </TooltipProvider>
</template>
