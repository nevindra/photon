<script setup>
// SearchBar — model A′ (spec §2/§3 of docs/superpowers/specs/2026-07-02-search-ux-revamp-design.md).
//
// Technique: a colored, pointer-events-none overlay <div> sits BEHIND a transparent real
// <input>. The input owns the text/caret/selection (100% normal text editing); the overlay
// renders the same string, tokenized into styled spans, kept in exact character alignment by
// sharing identical monospace font metrics + padding + border box, and scroll-synced on
// input/scroll. Pills get their breathing room via padding cancelled by an equal negative
// margin (`px-[3px] -mx-[3px]`) and a box-shadow ring (not a border) so NO glyph ever shifts —
// alignment holds regardless of pill/weight/strike/underline decoration (JetBrains Mono is
// monospaced, so semibold has the same advance as regular).
//
// Autocomplete is a custom listbox driven by the same lexer (`contextAt`); the input keeps DOM
// focus and navigation goes through `aria-activedescendant`. Everything is plain-text string
// splicing, so there is no contenteditable / mid-chip-edit failure mode.
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Search } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'
import { Kbd } from '@/components/ui/kbd'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { contextAt, termRangeAt, tokenize } from '@/lib/core/queryLang'
import { EXAMPLE_QUERIES, FIELDS } from '@/lib/logs/fields'

const props = defineProps({
  modelValue: { type: String, default: '' },
  error: { type: Object, default: null }, // { message: string, offset: number|null } | null
  services: { type: Array, default: () => [] },
  placeholder: { type: String, default: 'Search logs…' },
  // Field catalog + empty-state example queries, both defaulting to the logs catalog so
  // `LogsView` needs no change. Spans mode passes `SPAN_FIELDS`/`SPAN_EXAMPLE_QUERIES`
  // (see `lib/spanFields.js`) — same `{ name, description, kind, values? }` shape.
  catalog: { type: Array, default: () => FIELDS },
  exampleQueries: { type: Array, default: () => EXAMPLE_QUERIES },
})
const emit = defineEmits(['update:modelValue'])

// Unique ids so multiple SearchBars can't collide on aria wiring.
const uid = Math.random().toString(36).slice(2, 8)
const listboxId = `sb-lb-${uid}`

const inputEl = ref(null)
const overlayEl = ref(null)

// The overlay and the input MUST share these exact metrics (via inline style, so Tailwind
// utility ordering can never desync them). Border-box + a matching 1px border (transparent on
// the overlay) means both content boxes line up to the pixel; lineHeight === inner height
// vertically centers the single line.
const FIELD_FONT = "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, monospace"
const fieldMetrics = {
  fontFamily: FIELD_FONT,
  fontSize: '13px',
  lineHeight: '34px',
  letterSpacing: '0',
  paddingLeft: '32px',
  paddingRight: '12px',
  paddingTop: '0px',
  paddingBottom: '0px',
}

function safeTokenize(v) {
  try {
    return tokenize(v)
  } catch {
    return []
  }
}

// ---- Overlay rendering -----------------------------------------------------------------

// Group the flat token list into render "pieces": whitespace runs (rendered verbatim so the
// overlay stays character-aligned) and terms (consecutive non-whitespace tokens). A term is a
// pill iff it has a field key (field:value / compare / exists / negated variants); free-text
// and quoted terms render as bare green text, no pill box.
const pieces = computed(() => {
  const v = props.modelValue || ''
  if (!v) return []
  const tokens = safeTokenize(v)
  if (!tokens.length) return [{ type: 'plain', text: v }] // lexer failure → never break render
  const out = []
  let group = []
  const flush = () => {
    if (!group.length) return
    const hasField = group.some((t) => t.role === 'field')
    const negated = group.some((t) => t.negated)
    out.push({ type: 'term', tokens: group, isPill: hasField, negated })
    group = []
  }
  for (const t of tokens) {
    if (t.role === 'whitespace') {
      flush()
      out.push({ type: 'ws', text: t.text })
    } else {
      group.push(t)
    }
  }
  flush()
  return out
})

function termClass(term) {
  if (!term.isPill) return '' // free-text / quoted → no pill box
  const shell = 'rounded-[5px] px-[3px] -mx-[3px] py-[1px] ring-1 ring-inset'
  return term.negated ? cn(shell, 'ring-border bg-sev-error-soft') : cn(shell, 'ring-border bg-muted')
}

function tokenClass(tok) {
  const neg = tok.negated
  switch (tok.role) {
    case 'field':
      return neg ? 'font-semibold text-sev-error line-through' : 'font-semibold text-foreground'
    case 'operator':
      return neg ? 'text-sev-error' : 'text-muted-foreground'
    case 'value':
      return neg ? 'text-sev-error' : 'text-foreground'
    case 'negation':
      return 'text-sev-error'
    case 'quoted':
    case 'freetext':
      return neg ? 'text-sev-error' : 'text-green-700 dark:text-green-400'
    default:
      return ''
  }
}

// ---- Error state -----------------------------------------------------------------------

// Suppress error styling while the user is actively editing: hide on every `input`, and
// (re)show whenever the `error` prop transitions to a new value (parent only sets it after its
// debounced search resolves, so a fresh prop === "settled"). Initialised from the mount-time
// prop so an already-settled error passed on mount shows immediately.
const errorVisible = ref(!!props.error)
watch(
  () => props.error,
  (val) => {
    errorVisible.value = !!val
  },
)

// The char range to underline. Prefer the token containing `offset`; when offset is null or
// at/after the end (or sits in whitespace), underline the trailing term.
const errorRange = computed(() => {
  if (!errorVisible.value || !props.error) return null
  const v = props.modelValue || ''
  const toks = safeTokenize(v).filter((t) => t.role !== 'whitespace')
  if (!toks.length) return null
  const off = props.error.offset
  const last = toks[toks.length - 1]
  if (off == null) return termRangeAt(v, last.start)
  const hit = toks.find((t) => off >= t.start && off < t.end)
  if (hit) return { start: hit.start, end: hit.end }
  const r = termRangeAt(v, off)
  if (r.start !== r.end) return r
  return termRangeAt(v, last.start)
})

function isErrorTok(tok) {
  const r = errorRange.value
  return !!r && tok.role !== 'whitespace' && tok.start < r.end && tok.end > r.start
}

const errorColumn = computed(() => {
  const off = props.error?.offset
  return typeof off === 'number' ? off + 1 : null
})

// ---- Autocomplete ----------------------------------------------------------------------

const caret = ref(0)
const focused = ref(false)
const suppressed = ref(false) // Esc, or after a value/example insertion, until the next input
const activeIndex = ref(0)
// Whether the user has *explicitly* moved onto a suggestion (arrow key or hover) since the offered
// set last changed. Gates auto-accept on the default browse list (see `browsingDefault`): the row
// is visually highlighted at index 0 by default, but Enter shouldn't act on it until the user
// signals intent. A typed prefix bypasses this (completing a prefix on Enter is expected).
const activeNavigated = ref(false)

const context = computed(() => contextAt(props.modelValue || '', caret.value))

// The dropdown is showing the *default browse list* (every field + the example queries) rather
// than completing a token the user is typing — i.e. an empty term in field context (an empty
// query, or the caret right after a space). In that state Enter/Tab must NOT graft the first
// suggestion onto the query: pressing Enter with an empty box should apply the (empty) query, not
// fill in `body:` or an example. Only an explicit pick — arrowing/hovering to a row, or a typed
// prefix — counts as "select this". A `field:` value list is a deliberate completion, so it's
// excluded (kind === 'value' → not a browse list).
const browsingDefault = computed(
  () => context.value.kind === 'field' && (context.value.prefix || '') === '',
)

// Raw item list for the current caret context (no ids yet).
function buildItems() {
  const ctx = context.value
  if (ctx.kind === 'freetext') return []

  if (ctx.kind === 'value') {
    const f = props.catalog.find((field) => field.name === ctx.field)
    if (!f || !f.values) return []
    const src = f.values === 'services' ? props.services || [] : f.values
    const pfx = (ctx.prefix || '').toLowerCase()
    return src
      .filter((val) => typeof val === 'string' && val.toLowerCase().includes(pfx))
      .map((val) => ({ group: 'values', type: 'value', label: val, value: val, match: ctx.prefix }))
  }

  // kind === 'field'
  const pfx = (ctx.prefix || '').toLowerCase()
  const fields = props.catalog.filter((f) => f.name.toLowerCase().includes(pfx)).map((f) => ({
    group: 'fields',
    type: 'field',
    label: f.name,
    name: f.name,
    description: f.description,
    match: ctx.prefix,
  }))
  if ((ctx.prefix || '') === '') {
    const examples = props.exampleQueries.map((q) => ({ group: 'examples', type: 'example', label: q, query: q }))
    return [...fields, ...examples]
  }
  return fields
}

const items = computed(() => buildItems().map((it, i) => ({ ...it, id: `sb-opt-${uid}-${i}` })))

const open = computed(() => focused.value && !suppressed.value && items.value.length > 0)

// Whether a row should read as *actively selected* — highlighted, aria-selected, and the target
// of Enter. On the default browse list (empty term) nothing is selected until the user explicitly
// picks a row (arrow/hover): showing a highlighted item 0 there wrongly implies the system already
// chose it. When completing a typed prefix or a `field:` value list, the top row IS the Enter
// target, so it highlights normally.
const showActive = computed(() => !(browsingDefault.value && !activeNavigated.value))

const activeItem = computed(() => (open.value && showActive.value ? items.value[activeIndex.value] : null))
const activeDescendant = computed(() => activeItem.value?.id)

// Reset the active row (and the "user picked one" flag) whenever the offered set changes.
watch(
  () => items.value.map((i) => i.id).join('|') + '::' + context.value.kind,
  () => {
    activeIndex.value = 0
    activeNavigated.value = false
  },
)

// The listbox scrolls (max-h-72 overflow-y-auto); keep the active row visible as it changes.
watch(activeIndex, async () => {
  if (!open.value) return
  const id = activeItem.value?.id
  await nextTick()
  document.getElementById(id)?.scrollIntoView?.({ block: 'nearest' })
})

// Rows to render: option rows interleaved with (non-selectable) group headers.
const rows = computed(() => {
  const out = []
  let prev = null
  items.value.forEach((it, idx) => {
    if (it.group !== prev) {
      out.push({ kind: 'header', label: headerLabel(it.group), key: `h-${it.group}` })
      prev = it.group
    }
    out.push({ kind: 'option', item: it, index: idx })
  })
  return out
})

function headerLabel(group) {
  if (group === 'fields') return 'Fields'
  if (group === 'examples') return 'Examples'
  if (group === 'values') return context.value.field || 'Values'
  return ''
}

// Split a label around the (case-insensitive) matched prefix substring for highlighting.
function highlightParts(label, match) {
  if (!match) return [{ text: label, hit: false }]
  const i = label.toLowerCase().indexOf(match.toLowerCase())
  if (i === -1) return [{ text: label, hit: false }]
  return [
    { text: label.slice(0, i), hit: false },
    { text: label.slice(i, i + match.length), hit: true },
    { text: label.slice(i + match.length), hit: false },
  ].filter((p) => p.text !== '')
}

// ---- Insertion (plain string splice) ---------------------------------------------------

async function accept(item) {
  if (!item) return
  const v = props.modelValue || ''
  const c = caret.value
  let newValue
  let newCaret
  let keepOpen = false

  if (item.type === 'example') {
    newValue = item.query
    newCaret = newValue.length
  } else if (item.type === 'field') {
    const range = termRangeAt(v, c)
    const termText = v.slice(range.start, range.end)
    const neg = termText.startsWith('-') ? '-' : '' // preserve a leading "-" the user typed
    const insert = neg + item.name + ':'
    newValue = v.slice(0, range.start) + insert + v.slice(range.end)
    newCaret = range.start + insert.length
    keepOpen = true // leave the caret after the colon so value suggestions open next
  } else {
    // value: replace the current OR-segment (partial value) with `value ` and close.
    const prefix = context.value.prefix || ''
    const segStart = c - prefix.length
    const range = termRangeAt(v, c)
    let segEnd = range.end
    const nextComma = v.indexOf(',', c)
    if (nextComma !== -1 && nextComma < range.end) segEnd = nextComma
    const insert = `${item.value} `
    newValue = v.slice(0, segStart) + insert + v.slice(segEnd)
    newCaret = segStart + insert.length
  }

  suppressed.value = !keepOpen
  errorVisible.value = false
  emit('update:modelValue', newValue)
  await nextTick()
  caret.value = newCaret
  const el = inputEl.value
  if (el) {
    el.focus()
    try {
      el.setSelectionRange(newCaret, newCaret)
    } catch {
      /* setSelectionRange can throw on detached nodes in jsdom; ignore */
    }
    syncScroll()
  }
}

// ---- Input event plumbing --------------------------------------------------------------

function syncScroll() {
  const i = inputEl.value
  const o = overlayEl.value
  if (i && o) o.scrollLeft = i.scrollLeft
}

// A layout resize can change the input's scroll offset without firing `scroll`/`input`, which
// would desync the overlay. Watch the input element itself (not window/document — this
// component deliberately keeps zero global listeners). Guard the constructor's existence for
// environments without it (e.g. jsdom in tests).
let resizeObserver = null

onMounted(() => {
  if (inputEl.value && typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(syncScroll)
    resizeObserver.observe(inputEl.value)
  }
})

onUnmounted(() => {
  resizeObserver?.disconnect()
})

function updateCaret() {
  const el = inputEl.value
  if (el) caret.value = el.selectionStart ?? 0
  syncScroll()
}

function onInput(e) {
  const el = e.target
  errorVisible.value = false // suppress error visuals while actively editing
  suppressed.value = false
  activeNavigated.value = false // fresh text = a new completion, not a picked row
  emit('update:modelValue', el.value)
  caret.value = el.selectionStart ?? el.value.length
  nextTick(syncScroll)
}

function onFocus() {
  focused.value = true
  suppressed.value = false
  activeNavigated.value = false // opening the list is browsing, not yet a pick
  updateCaret()
}

// Pointing at a row (hover) is an explicit pick, same as arrowing to it.
function hoverOption(index) {
  activeIndex.value = index
  activeNavigated.value = true
}

function onBlur() {
  // Options use @mousedown.prevent, so choosing one does not blur the input; a genuine
  // blur (click elsewhere / tab away) closes the dropdown.
  focused.value = false
}

function move(delta) {
  const n = items.value.length
  if (!n) return
  // Reopen a dismissed dropdown, or make the FIRST move land on a natural end (top for Down,
  // bottom for Up) — there's no highlighted row to step off of until the user has picked one.
  if (!open.value || !showActive.value) {
    suppressed.value = false
    activeNavigated.value = true // an arrow key is an explicit pick
    activeIndex.value = delta > 0 ? 0 : n - 1
    return
  }
  activeNavigated.value = true
  activeIndex.value = (activeIndex.value + delta + n) % n
}

function onKeydown(e) {
  switch (e.key) {
    case 'ArrowDown':
      // Only hijack the key when there's something to navigate — otherwise let the input's
      // native caret-to-line-end behavior through untouched.
      if (open.value && items.value.length) {
        e.preventDefault()
        move(1)
      }
      break
    case 'ArrowUp':
      if (open.value && items.value.length) {
        e.preventDefault()
        move(-1)
      }
      break
    case 'Enter':
      if (open.value) {
        e.preventDefault()
        if (browsingDefault.value && !activeNavigated.value) {
          // Empty term + no explicit pick → apply the query as-is (don't graft the top row);
          // just dismiss the browse list. This is the "clear the box, press Enter" case.
          suppressed.value = true
        } else {
          accept(items.value[activeIndex.value])
        }
      }
      break
    case 'Tab':
      if (open.value && !(browsingDefault.value && !activeNavigated.value)) {
        e.preventDefault() // don't blur; accept instead
        accept(items.value[activeIndex.value])
      }
      // else: nothing to complete → let Tab move focus (the dropdown closes on blur)
      break
    case 'Escape':
      if (open.value) {
        e.preventDefault()
        suppressed.value = true // close without clearing the query
      }
      break
    default:
      break
  }
}
</script>

<template>
  <div class="relative w-full">
    <div
      class="relative w-full rounded-md border border-input bg-background shadow-sink transition-shadow focus-within:ring-2 focus-within:ring-ring"
    >
      <!-- Overlay (behind): tokenized, styled, character-aligned, non-interactive. -->
      <div
        ref="overlayEl"
        aria-hidden="true"
        class="pointer-events-none absolute inset-0 overflow-hidden whitespace-pre rounded-md border border-transparent text-foreground"
        :style="fieldMetrics"
      >
        <template v-for="(piece, pi) in pieces" :key="pi">
          <span v-if="piece.type === 'ws'">{{ piece.text }}</span>
          <span v-else-if="piece.type === 'plain'">{{ piece.text }}</span>
          <!-- Pill term: field key / operator / value wrapped in a rounded pill shell. -->
          <span v-else-if="piece.isPill" :class="termClass(piece)">
            <span
              v-for="(tok, ti) in piece.tokens"
              :key="ti"
              :class="[
                tokenClass(tok),
                isErrorTok(tok) && 'underline decoration-sev-error decoration-wavy underline-offset-2',
              ]"
              >{{ tok.text }}</span
            >
          </span>
          <!-- Free-text / quoted term: no pill box, tokens rendered directly. -->
          <template v-else>
            <span
              v-for="(tok, ti) in piece.tokens"
              :key="ti"
              :class="[
                tokenClass(tok),
                isErrorTok(tok) && 'underline decoration-sev-error decoration-wavy underline-offset-2',
              ]"
              >{{ tok.text }}</span
            >
          </template>
        </template>
      </div>

      <!-- Real input (front): transparent text, visible caret; owns editing + selection. -->
      <input
        ref="inputEl"
        type="text"
        role="combobox"
        aria-label="Search logs"
        aria-autocomplete="list"
        :aria-expanded="open"
        :aria-controls="listboxId"
        :aria-activedescendant="activeDescendant"
        autocomplete="off"
        autocapitalize="off"
        autocorrect="off"
        spellcheck="false"
        :value="modelValue"
        :placeholder="placeholder"
        :style="fieldMetrics"
        :class="
          cn(
            'relative block h-9 w-full rounded-md border border-transparent bg-transparent text-transparent caret-brand outline-none transition-colors placeholder:text-muted-foreground',
          )
        "
        @input="onInput"
        @keydown="onKeydown"
        @keyup="updateCaret"
        @click="updateCaret"
        @select="updateCaret"
        @scroll="syncScroll"
        @focus="onFocus"
        @blur="onBlur"
      >

      <!-- Leading search glyph (on top, in the left padding gutter). -->
      <Search
        class="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
      />

      <!-- Autocomplete dropdown: scrollable listbox + a persistent keyboard-hint footer. -->
      <div
        v-if="open"
        class="absolute left-0 right-0 top-full z-50 mt-1 overflow-hidden rounded-md border border-border bg-surface-2 text-popover-foreground shadow-2"
      >
        <ul :id="listboxId" role="listbox" class="max-h-72 overflow-y-auto p-1">
        <template v-for="row in rows" :key="row.key ?? row.item.id">
          <li
            v-if="row.kind === 'header'"
            role="presentation"
            class="px-2 pb-1 pt-2 text-[10px] font-medium uppercase tracking-wide text-muted-foreground"
          >
            {{ row.label }}
          </li>
          <li
            v-else
            :id="row.item.id"
            role="option"
            :aria-selected="showActive && row.index === activeIndex"
            :class="
              cn(
                'flex cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm',
                // Muted-but-visible highlight: the theme's --accent/--muted are 96.1% in light
                // mode, only ~4% off the white popover, so they read as invisible. Explicit
                // neutral grays give a soft, clearly-visible selected state in both themes.
                // Gated on `showActive` so nothing looks pre-selected on the default browse list.
                showActive && row.index === activeIndex && 'bg-neutral-200 text-foreground dark:bg-neutral-800',
              )
            "
            @mouseenter="hoverOption(row.index)"
            @mousedown.prevent
            @click="accept(row.item)"
          >
            <span class="truncate font-mono text-xs">
              <span
                v-for="(part, pj) in highlightParts(row.item.label, row.item.match)"
                :key="pj"
                :class="part.hit ? 'font-semibold text-foreground' : 'text-foreground'"
                >{{ part.text }}</span
              >
            </span>
            <span
              v-if="row.item.description"
              class="ml-auto truncate text-xs text-muted-foreground"
              >{{ row.item.description }}</span
            >
          </li>
        </template>
        </ul>
        <!-- Keyboard-hint footer — always visible so users discover keyboard navigation. -->
        <div
          class="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border bg-muted/40 px-2.5 py-1.5 text-[10px] text-muted-foreground"
        >
          <span class="flex items-center gap-1"><Kbd>↑</Kbd><Kbd>↓</Kbd><span class="ml-0.5">navigate</span></span>
          <span class="flex items-center gap-1"><Kbd>Tab</Kbd><span class="text-muted-foreground/50">/</span><Kbd>↵</Kbd><span class="ml-0.5">select</span></span>
          <span class="flex items-center gap-1"><Kbd>Esc</Kbd><span class="ml-0.5">dismiss</span></span>
        </div>
      </div>
    </div>

    <!-- Error line, directly under the bar (post-debounce only). -->
    <Alert v-if="errorVisible && error" variant="error" class="mt-1">
      <AlertDescription>
        <span aria-hidden="true">⚠</span>
        <span class="ml-1">{{ error.message }}<template v-if="errorColumn"> (column {{ errorColumn }})</template></span>
      </AlertDescription>
    </Alert>
  </div>
</template>
