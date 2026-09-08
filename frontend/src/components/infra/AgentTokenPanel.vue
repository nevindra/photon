<script setup lang="ts">
// The minted-token panel: shown ONCE after enrolling or rotating, with the install snippet the
// operator pastes on the host. Same panel idiom as the federation tenant dialog, because it is
// the same moment — a secret that will not be shown again.
import { ref } from 'vue'
import { Copy } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { useCopy } from '@/lib/core/useCopy'
import { installSnippet, type InstallFormat } from '@/lib/deploy/agentStatus'
import type { AgentCapability } from '@/lib/core/api'

const props = defineProps<{
  token: string
  host: string
  capabilities: AgentCapability[]
  pollSecs: number
  /** "rotated" changes the copy: the old token stops working on the next sync. */
  rotated?: boolean
}>()

const { copy } = useCopy()
const format = ref<InstallFormat>('systemd')
const origin = typeof location !== 'undefined' ? location.origin : 'https://photon.example.com'
const snippet = () => installSnippet({ token: props.token, host: props.host, capabilities: props.capabilities, pollSecs: props.pollSecs, origin }, format.value)
</script>

<template>
  <div class="rounded-lg border border-brand/40 bg-brand/5 p-3" data-testid="agent-token-panel">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <p class="text-xs font-medium text-foreground">
        Agent token for <span class="font-mono">{{ host }}</span> — shown once, copy it now.
        <span v-if="rotated" class="block font-normal text-muted-foreground">Re-run the install on the host with this token; the old one stops working on the next sync.</span>
      </p>
      <div class="flex shrink-0 items-center gap-1">
        <Segmented v-model="format" class="h-7" data-testid="agent-snippet-format">
          <SegmentedItem value="systemd" class="px-2 text-xs">systemd</SegmentedItem>
          <SegmentedItem value="docker" class="px-2 text-xs">docker run</SegmentedItem>
        </Segmented>
        <Button type="button" variant="outline" size="sm" class="h-7 px-2 text-xs" data-testid="agent-copy-token" @click="copy(token, 'token')">
          <Copy class="size-3.5" /> Token
        </Button>
        <Button type="button" variant="outline" size="sm" class="h-7 px-2 text-xs" data-testid="agent-copy-snippet" @click="copy(snippet(), 'install snippet')">
          <Copy class="size-3.5" /> Snippet
        </Button>
      </div>
    </div>
    <pre class="mt-2 max-h-56 overflow-auto whitespace-pre-wrap break-all rounded bg-surface-2 p-2 font-mono text-xs">{{ snippet() }}</pre>
    <p class="mt-2 text-[11px] text-muted-foreground">
      The binary is served by this Photon, so the install works on an air-gapped site. Capabilities are flags on the host: to change them later, re-run the install.
    </p>
  </div>
</template>
