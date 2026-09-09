<script setup lang="ts">
// The minted-token panel: shown ONCE after enrolling or rotating, with the install the operator
// runs on the host. Same moment as the federation tenant dialog — a secret that will not be
// shown again — but the job is bigger than "copy this string": the operator has to get a
// binary onto a machine, run it as the right user, and know where it should call home. So the
// panel is three numbered steps, each saying what can go wrong BEFORE anything is pasted:
//
// 1. the token, copyable, with the one-time warning;
// 2. the URL the host will dial — the server's own idea of it, editable, with a warning when it
//    is loopback (only works on this machine), plain http (the token travels in clear), or when
//    this Photon has no agent binary to hand out (the first line would 404);
// 3. the command, as a service or in the foreground, token prompted for by default.
import { computed, ref, watch } from 'vue'
import { Copy, Check, TriangleAlert, Info } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { useCopy } from '@/lib/core/useCopy'
import { cn } from '@/lib/core/utils'
import { installSnippet, originIssue, normalizeOrigin, INSTALL_FORMAT_META, type InstallFormat } from '@/lib/deploy/agentStatus'
import type { AgentCapability } from '@/lib/core/api'

const props = defineProps<{
  token: string
  host: string
  capabilities: AgentCapability[]
  pollSecs: number
  /** The URL the server thinks agents should dial. Falls back to this UI's origin. */
  serverUrl?: string | null
  /** Architectures this Photon can serve a binary for; undefined = unknown (older server). */
  agentArchs?: string[] | null
  /** "rotated" changes the copy: the old token stops working on the next sync. */
  rotated?: boolean
}>()

const { copy } = useCopy()
const format = ref<InstallFormat>('systemd')
const embedToken = ref(false)
const origin = ref(normalizeOrigin(props.serverUrl || (typeof location !== 'undefined' ? location.origin : 'https://photon.example.com')))
watch(
  () => props.serverUrl,
  (u) => {
    if (u) origin.value = normalizeOrigin(u)
  },
)

const issue = computed(() => originIssue(origin.value))
const snippet = computed(() =>
  installSnippet({ token: props.token, host: props.host, capabilities: props.capabilities, pollSecs: props.pollSecs, origin: origin.value, embedToken: embedToken.value }, format.value),
)
const noBinary = computed(() => Array.isArray(props.agentArchs) && props.agentArchs.length === 0)

// A short-lived "Copied" state on the snippet button — the toast fires too, but the eye is on
// the button that was just pressed.
const copiedSnippet = ref(false)
async function copySnippet(): Promise<void> {
  await copy(snippet.value, 'install command')
  copiedSnippet.value = true
  setTimeout(() => (copiedSnippet.value = false), 1600)
}
const copiedToken = ref(false)
async function copyToken(): Promise<void> {
  await copy(props.token, 'token')
  copiedToken.value = true
  setTimeout(() => (copiedToken.value = false), 1600)
}

const FORMATS: InstallFormat[] = ['systemd', 'foreground']
</script>

<template>
  <div class="flex flex-col gap-4" data-testid="agent-token-panel">
    <!-- 1. The token -->
    <section class="rounded-xl border border-brand/40 bg-brand-soft/60 p-4" data-testid="agent-token-step">
      <div class="flex items-start gap-3">
        <span class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full bg-brand text-xs font-semibold text-brand-foreground">1</span>
        <div class="min-w-0 flex-1">
          <p class="text-sm font-semibold text-foreground">Copy the agent token for <span class="font-mono">{{ host }}</span></p>
          <p class="mt-0.5 text-xs text-muted-foreground">
            <template v-if="rotated">The old token stops working on the host's next sync. Re-run the install below with this one.</template>
            <template v-else>Shown once. Leave this screen and it is gone — you can rotate a new one from the host's Agent tab.</template>
          </p>
          <div class="mt-2.5 flex items-stretch gap-2">
            <code class="flex min-w-0 flex-1 items-center overflow-x-auto whitespace-nowrap rounded-md border border-border bg-surface-1 px-3 font-mono text-sm text-foreground" data-testid="agent-token-value">{{ token }}</code>
            <Button type="button" variant="outline" class="shrink-0 gap-1.5" data-testid="agent-copy-token" @click="copyToken">
              <Check v-if="copiedToken" class="size-4 text-success" /><Copy v-else class="size-4" /> {{ copiedToken ? 'Copied' : 'Copy token' }}
            </Button>
          </div>
        </div>
      </div>
    </section>

    <!-- 2. Where the host reaches this Photon -->
    <section class="rounded-xl border border-border bg-card p-4" data-testid="agent-origin-step">
      <div class="flex items-start gap-3">
        <span class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold text-foreground">2</span>
        <div class="min-w-0 flex-1">
          <label class="text-sm font-semibold text-foreground" for="agent-origin">Check the URL the host will use to reach this Photon</label>
          <p class="mt-0.5 text-xs text-muted-foreground">The agent downloads itself from here and syncs here. It must resolve from the host's network, not just from your browser.</p>
          <Input id="agent-origin" v-model="origin" class="mt-2.5 font-mono" data-testid="agent-origin" autocomplete="off" spellcheck="false" placeholder="https://photon.example.com" />
          <p v-if="issue === 'invalid'" class="mt-2 flex items-start gap-1.5 text-xs text-sev-error" data-testid="agent-origin-issue" data-issue="invalid">
            <TriangleAlert class="mt-0.5 size-3.5 shrink-0" /> Enter a full URL starting with http:// or https://.
          </p>
          <p v-else-if="issue === 'loopback'" class="mt-2 flex items-start gap-1.5 text-xs text-sev-warn" data-testid="agent-origin-issue" data-issue="loopback">
            <TriangleAlert class="mt-0.5 size-3.5 shrink-0" />
            <span>This is a loopback address, so it only works for an agent running on this same machine. For any other host, replace it with an address that host can reach, or set <code class="font-mono">[deploy].server_origin</code> on the server so it is filled in for you.</span>
          </p>
          <p v-else-if="issue === 'plain-http'" class="mt-2 flex items-start gap-1.5 text-xs text-muted-foreground" data-testid="agent-origin-issue" data-issue="plain-http">
            <Info class="mt-0.5 size-3.5 shrink-0" /> Plain http: the token and everything the agent reports travel unencrypted. Fine on a trusted LAN; use https for anything that crosses the internet.
          </p>
          <p v-if="noBinary" class="mt-2 flex items-start gap-1.5 text-xs text-sev-error" data-testid="agent-no-binary">
            <TriangleAlert class="mt-0.5 size-3.5 shrink-0" />
            <span>This Photon has no <code class="font-mono">photon-agent</code> binary to hand out, so the download step below will fail. Build one and place it under the agent binaries directory (<code class="font-mono">[deploy].agent_binaries_dir</code>), or install the agent from a release tarball.</span>
          </p>
          <p v-else-if="agentArchs?.length" class="mt-2 text-xs text-muted-foreground" data-testid="agent-archs">
            Binary served for <span class="font-mono">{{ agentArchs.join(' · ') }}</span>. A host of another architecture needs a release tarball instead.
          </p>
        </div>
      </div>
    </section>

    <!-- 3. Run it on the host -->
    <section class="rounded-xl border border-border bg-card p-4" data-testid="agent-command-step">
      <div class="flex items-start gap-3">
        <span class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold text-foreground">3</span>
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-start justify-between gap-2">
            <div>
              <p class="text-sm font-semibold text-foreground">Run this on <span class="font-mono">{{ host }}</span></p>
              <p class="mt-0.5 text-xs text-muted-foreground">{{ INSTALL_FORMAT_META[format].blurb }}</p>
            </div>
            <Segmented v-model="format" class="h-8 shrink-0" data-testid="agent-snippet-format">
              <SegmentedItem v-for="f in FORMATS" :key="f" :value="f" class="px-2.5 text-xs">{{ INSTALL_FORMAT_META[f].label }}</SegmentedItem>
            </Segmented>
          </div>

          <div class="mt-2.5 overflow-hidden rounded-lg border border-border bg-surface-2">
            <div class="flex items-center justify-between gap-2 border-b border-border px-3 py-1.5">
              <span class="font-mono text-[11px] uppercase tracking-wide text-muted-foreground">bash · run as {{ format === 'systemd' ? 'a user with sudo' : 'any user' }}</span>
              <Button type="button" variant="ghost" size="sm" class="h-7 gap-1.5 px-2 text-xs" data-testid="agent-copy-snippet" @click="copySnippet">
                <Check v-if="copiedSnippet" class="size-3.5 text-success" /><Copy v-else class="size-3.5" /> {{ copiedSnippet ? 'Copied' : 'Copy' }}
              </Button>
            </div>
            <pre class="max-h-64 overflow-auto whitespace-pre-wrap break-words p-3 font-mono text-xs leading-relaxed text-foreground" data-testid="agent-snippet">{{ snippet }}</pre>
          </div>

          <label :class="cn('mt-2.5 flex cursor-pointer items-start gap-2 text-xs', embedToken ? 'text-foreground' : 'text-muted-foreground')" for="agent-embed-token">
            <Checkbox id="agent-embed-token" :model-value="embedToken" class="mt-0.5" data-testid="agent-embed-token" @update:model-value="embedToken = $event === true" />
            <span>
              Put the token in the command instead of typing it at the prompt.
              <span v-if="embedToken" class="block text-sev-warn">It will land in the shell history of whoever pastes it.</span>
              <span v-else class="block">Recommended off: the agent asks for it and hides the input, so it never reaches shell history.</span>
            </span>
          </label>

          <ul class="mt-3 flex flex-col gap-1 border-t border-border pt-3 text-xs text-muted-foreground">
            <li>The host appears under Hosts once its first sync lands, usually within a poll interval ({{ pollSecs }} s).</li>
            <li>Capabilities are flags on the host. To change them later, re-run the install with different flags.</li>
            <li v-if="format === 'foreground' && capabilities.length">Docker capabilities in the foreground need a user that can reach the Docker socket (the <code class="font-mono">docker</code> group, or root).</li>
          </ul>
        </div>
      </div>
    </section>
  </div>
</template>
