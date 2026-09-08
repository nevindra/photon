<script setup>
// Compact toolbar dropdown-select, built on the (jsdom-testable) Popover primitive rather than the
// Reka `Select` listbox — Reka Select needs pointer-capture APIs jsdom lacks, so it can't be driven
// in tests. The trigger shows an optional muted `prefix` + the current option's label + a chevron;
// opening reveals a small menu of options that emit `update:modelValue` and close on choose. One
// place for the toolbar-select look (Sort, refresh-mode, …) so they all read alike. `size="md"` is
// the same control sized as a form field — the trigger matches `Input` exactly (h-9, px-3, text-sm,
// full width) so a select sits level with the inputs beside it in a dialog form.
//
// The root is the renderless Reka Popover, so the class/id a caller passes would fall off it:
// attrs are forwarded to the trigger, where a FormField's `for=` and sizing classes actually belong.
// The id needs one more step: Reka mints its own trigger id and points the content's
// `aria-labelledby` at it, so a caller's id is seeded into the popover root context (Reka only
// mints one when that field is empty) — the trigger and the content then agree on the caller's id.
import { ref, computed, defineComponent, useAttrs } from "vue";
import { injectPopoverRootContext } from "reka-ui";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";

const SeedTriggerId = defineComponent({
  props: { id: { type: String, default: "" } },
  setup(props, { slots }) {
    const ctx = injectPopoverRootContext();
    if (props.id) ctx.triggerId = props.id;
    return () => slots.default?.();
  },
});
import { Check, ChevronDown } from "lucide-vue-next";
import { cn } from "@/lib/core/utils";

const props = defineProps({
  modelValue: { type: [String, Number], default: null },
  // The choices, in display order: [{ value, label }].
  options: { type: Array, default: () => [] },
  // Optional muted label shown before the value on the trigger, e.g. "Sort:".
  prefix: { type: String, default: "" },
  align: { type: String, default: "start" },
  contentClass: { type: String, default: "w-40" },
  ariaLabel: { type: String, default: "" },
  // 'sm' = the compact toolbar trigger (h-7, mono value); 'md' = a form field, sized like `Input`.
  size: { type: String, default: "sm" },
});
defineOptions({ inheritAttrs: false });
const emit = defineEmits(["update:modelValue"]);

const attrs = useAttrs();
const triggerAttrs = computed(() => {
  const { id: _id, ...rest } = attrs;
  return rest;
});

const open = ref(false);
const currentLabel = computed(
  () => props.options.find((o) => o.value === props.modelValue)?.label ?? "",
);

function choose(value) {
  if (value !== props.modelValue) emit("update:modelValue", value);
  open.value = false;
}
</script>

<template>
  <Popover v-model:open="open">
    <SeedTriggerId :id="attrs.id ? String(attrs.id) : ''">
      <PopoverTrigger
        v-bind="triggerAttrs"
        :aria-label="ariaLabel || prefix || 'Select'"
        :class="
          cn(
            'inline-flex items-center whitespace-nowrap rounded-md border bg-surface-1 text-foreground focus-visible:outline-none',
            size === 'md'
              ? 'h-9 w-full justify-between gap-2 border-input px-3 text-sm transition-[border-color,box-shadow] duration-150 ease-out hover:border-foreground/15 focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/20 data-[state=open]:border-ring'
              : 'h-7 gap-1.5 border-border px-2.5 text-xs font-medium shadow-1 transition-[background-color,border-color] duration-150 ease-out hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring data-[state=open]:bg-muted',
            attrs.class,
          )
        "
      >
        <span v-if="prefix" class="text-muted-foreground">{{ prefix }}</span>
        <span :class="size === 'md' ? 'truncate' : 'font-mono'">{{
          currentLabel
        }}</span>
        <ChevronDown
          :class="
            cn('shrink-0 opacity-50', size === 'md' ? 'size-4' : 'size-3.5')
          "
        />
      </PopoverTrigger>
    </SeedTriggerId>
    <PopoverContent :align="align" :class="cn('p-1', contentClass)">
      <div class="flex flex-col gap-0.5">
        <button
          v-for="o in options"
          :key="o.value"
          type="button"
          :data-value="o.value"
          :data-testid="`select-option-${o.value}`"
          :aria-selected="o.value === modelValue"
          class="flex cursor-pointer items-center justify-between gap-3 rounded-sm px-2 py-1 text-left font-mono text-xs text-foreground hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:outline-none"
          @click="choose(o.value)"
        >
          <span class="truncate">{{ o.label }}</span>
          <Check v-if="o.value === modelValue" class="size-3.5 shrink-0" />
        </button>
      </div>
    </PopoverContent>
  </Popover>
</template>
