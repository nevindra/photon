<script setup lang="ts">
// A shell inside a container — the single highest-risk feature in the product, so it ships last,
// behind its own agent capability (`--docker-exec`) and its own role (admin), and every session
// leaves an audit row with who, which container, how long, and how many commands. The session is
// agent-initiated like everything else: no port opens on the host. Secret-looking environment
// values come back masked.
import { computed, nextTick, onBeforeUnmount, ref } from 'vue'
import { Terminal, Loader2, ShieldAlert, Square } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { api, type ExecSession } from '@/lib/core/api'
import { toast } from '@/components/ui/toast'
import { useQueryClient } from '@tanstack/vue-query'
import { auditKey } from '@/lib/deploy/deployQueries'

const props = defineProps<{ host: string; containerId: string; containerName: string; blocked: string | null }>()

const qc = useQueryClient()
const session = ref<ExecSession | null>(null)
const opening = ref(false)
const busy = ref(false)
const closed = ref(false)
interface Line {
  kind: 'cmd' | 'out' | 'err' | 'sys'
  text: string
}
const lines = ref<Line[]>([])
const input = ref('')
const history = ref<string[]>([])
const historyIdx = ref(-1)
const scroller = ref<HTMLElement | null>(null)
const field = ref<HTMLInputElement | null>(null)
const prompt = computed(() => `root@${session.value?.container_id.slice(0, 12) ?? '…'}:~#`)

async function open(): Promise<void> {
  opening.value = true
  const res = await api.openExec(props.host, props.containerId)
  opening.value = false
  if (!res.ok || !res.session) {
    toast({ variant: 'error', title: "Couldn't open a shell", description: res.error ?? 'Please try again.' })
    return
  }
  session.value = res.session
  closed.value = false
  lines.value = [{ kind: 'sys', text: `Connected to ${res.session.container_name} on ${props.host} (${res.session.shell}). This session is audited.` }]
  qc.invalidateQueries({ queryKey: auditKey() })
  await nextTick()
  field.value?.focus()
}
async function send(): Promise<void> {
  if (!session.value || busy.value) return
  const line = input.value
  input.value = ''
  if (line.trim()) {
    history.value = [line, ...history.value].slice(0, 50)
  }
  historyIdx.value = -1
  lines.value.push({ kind: 'cmd', text: line })
  busy.value = true
  let out = await api.execSend(session.value.id, line)
  // The server answers with what the shell printed within ~1.5s. A slow command (or the very
  // first one, while the exec is still starting) answers later: an empty line is a poll for
  // that late output, so keep asking while nothing has come back yet, a few times at most.
  const answered = (o: typeof out): boolean => o.answered ?? o.lines.length > 0
  const show = (o: typeof out): void => {
    for (const t of o.lines) lines.value.push({ kind: o.exit_code && o.exit_code !== 0 ? 'err' : 'out', text: t })
  }
  show(out)
  for (let polls = 0; !answered(out) && !out.closed && polls < 3; polls++) {
    out = await api.execSend(session.value.id, '')
    show(out)
  }
  busy.value = false
  if (out.closed) {
    closed.value = true
    lines.value.push({ kind: 'sys', text: 'Session closed.' })
    session.value = null
    qc.invalidateQueries({ queryKey: auditKey() })
  }
  await nextTick()
  if (scroller.value) scroller.value.scrollTop = scroller.value.scrollHeight
}
async function close(): Promise<void> {
  if (!session.value) return
  await api.closeExec(session.value.id)
  session.value = null
  closed.value = true
  lines.value.push({ kind: 'sys', text: 'Session closed.' })
  qc.invalidateQueries({ queryKey: auditKey() })
}
function onKey(e: KeyboardEvent): void {
  if (e.key === 'ArrowUp') {
    e.preventDefault()
    historyIdx.value = Math.min(historyIdx.value + 1, history.value.length - 1)
    input.value = history.value[historyIdx.value] ?? ''
  } else if (e.key === 'ArrowDown') {
    e.preventDefault()
    historyIdx.value = Math.max(historyIdx.value - 1, -1)
    input.value = historyIdx.value === -1 ? '' : history.value[historyIdx.value]
  } else if (e.key === 'c' && e.ctrlKey) {
    e.preventDefault()
    input.value = ''
  } else if (e.key === 'd' && e.ctrlKey) {
    e.preventDefault()
    void close()
  }
}
// Leaving the tab ends the session: a shell nobody is looking at is a shell nobody should hold.
onBeforeUnmount(() => {
  if (session.value) void api.closeExec(session.value.id)
})
</script>

<template>
  <div class="flex h-full min-h-0 flex-col" data-testid="container-shell">
    <div v-if="!session" class="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
      <span class="flex size-10 items-center justify-center rounded-lg bg-muted text-muted-foreground"><Terminal class="size-5" /></span>
      <p class="text-sm text-foreground">Open a shell in <span class="font-mono">{{ containerName }}</span></p>
      <p class="max-w-md text-xs text-muted-foreground">Root inside the container, over the agent's own outbound stream — no port opens on the host. Every session is recorded in the audit log: who, which container, how long, how many commands.</p>
      <p v-if="blocked" class="flex items-center gap-1.5 text-xs text-sev-warn" data-testid="shell-blocked"><ShieldAlert class="size-3.5" /> {{ blocked }}</p>
      <Button v-else :disabled="opening" data-testid="shell-open" @click="open"><Loader2 v-if="opening" class="animate-spin" /><Terminal v-else class="size-4" /> {{ closed ? 'Open another shell' : 'Open shell' }}</Button>
      <div v-if="closed && lines.length" class="mt-2 w-full max-w-2xl text-left">
        <pre class="max-h-48 overflow-auto rounded-lg border border-border bg-surface-1 p-3 font-mono text-[11.5px] leading-5 text-muted-foreground">{{ lines.map((l) => (l.kind === 'cmd' ? '$ ' : '') + l.text).join('\n') }}</pre>
      </div>
    </div>
    <template v-else>
      <div class="flex items-center gap-2 border-b border-border px-4 py-2 text-xs">
        <span class="inline-flex items-center gap-1.5 text-success"><span class="size-1.5 animate-pulse rounded-full bg-success" /> live</span>
        <span class="font-mono text-muted-foreground">{{ session.id }}</span>
        <span class="text-muted-foreground">· audited</span>
        <Button variant="outline" size="sm" class="ml-auto h-7 px-2 text-xs" data-testid="shell-close" @click="close"><Square class="size-3.5" /> End session</Button>
      </div>
      <div ref="scroller" class="min-h-0 flex-1 overflow-auto bg-[hsl(240_8%_9%)] px-3 py-2 font-mono text-[12px] leading-5 text-[hsl(0_0%_88%)]" data-testid="shell-output" @click="field?.focus()">
        <div v-for="(l, i) in lines" :key="i" :class="l.kind === 'cmd' ? 'text-[hsl(190_90%_70%)]' : l.kind === 'err' ? 'text-[hsl(0_84%_72%)]' : l.kind === 'sys' ? 'text-[hsl(240_5%_60%)] italic' : ''" class="whitespace-pre-wrap break-all">{{ l.kind === 'cmd' ? prompt + ' ' + l.text : l.text }}</div>
        <form class="flex items-center gap-2" @submit.prevent="send">
          <span class="shrink-0 text-[hsl(190_90%_70%)]">{{ prompt }}</span>
          <input ref="field" v-model="input" class="min-w-0 flex-1 bg-transparent text-[hsl(0_0%_95%)] outline-none" autocomplete="off" autocapitalize="off" spellcheck="false" :disabled="busy" data-testid="shell-input" @keydown="onKey" />
          <Loader2 v-if="busy" class="size-3 animate-spin text-[hsl(240_5%_60%)]" />
        </form>
      </div>
      <div class="border-t border-border px-4 py-1.5 text-[11px] text-muted-foreground">↑ ↓ history · Ctrl-C clears the line · Ctrl-D or <code class="font-mono">exit</code> ends the session</div>
    </template>
  </div>
</template>
