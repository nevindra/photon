import { describe, it, expect } from "vitest";
import { nextTick } from "vue";
import { mount, DOMWrapper } from "@vue/test-utils";
import { SelectMenu } from "./index.js";

const OPTIONS = [
  { value: "recent", label: "Recent" },
  { value: "slowest", label: "Slowest" },
  { value: "errors", label: "Errors" },
];

function mountMenu(props = {}, attrs = {}) {
  return mount(SelectMenu, {
    props: { options: OPTIONS, modelValue: "recent", ...props },
    attrs,
    attachTo: document.body,
  });
}

// PopoverContent teleports to document.body; open it, then query the body — the same pattern
// ColumnPicker/TraceTable tests use.
async function open(w) {
  await w.get("[aria-label]").trigger("click");
  await nextTick();
  await new Promise((r) => setTimeout(r, 0));
  return new DOMWrapper(document.body);
}

describe("SelectMenu", () => {
  it("shows the current option label (and prefix) on the trigger", () => {
    const w = mountMenu({ prefix: "Sort:" });
    expect(w.text()).toContain("Sort:");
    expect(w.text()).toContain("Recent");
    w.unmount();
  });

  it("opens on trigger click and lists every option", async () => {
    const w = mountMenu();
    const body = await open(w);
    for (const o of OPTIONS) {
      expect(
        body.find(`[data-testid="select-option-${o.value}"]`).exists(),
      ).toBe(true);
    }
    w.unmount();
  });

  it("emits update:modelValue with the chosen value", async () => {
    const w = mountMenu();
    const body = await open(w);
    await body.find('[data-testid="select-option-slowest"]').trigger("click");
    expect(w.emitted("update:modelValue")[0]).toEqual(["slowest"]);
    w.unmount();
  });

  it("does not re-emit when the already-selected value is chosen", async () => {
    const w = mountMenu({ modelValue: "recent" });
    const body = await open(w);
    await body.find('[data-testid="select-option-recent"]').trigger("click");
    expect(w.emitted("update:modelValue")).toBeUndefined();
    w.unmount();
  });

  it("marks the selected option (aria-selected) for the check indicator", async () => {
    const w = mountMenu({ modelValue: "slowest" });
    const body = await open(w);
    expect(
      body
        .find('[data-testid="select-option-slowest"]')
        .attributes("aria-selected"),
    ).toBe("true");
    expect(
      body
        .find('[data-testid="select-option-recent"]')
        .attributes("aria-selected"),
    ).toBe("false");
    w.unmount();
  });

  it('is the compact toolbar trigger by default and a field-sized one at size="md"', () => {
    const sm = mountMenu();
    expect(sm.get("[aria-label]").classes()).toContain("h-7");
    sm.unmount();
    const md = mountMenu({ size: "md" });
    const trigger = md.get("[aria-label]");
    expect(trigger.classes()).toContain("h-9");
    expect(trigger.classes()).toContain("w-full");
    expect(trigger.classes()).not.toContain("h-7");
    md.unmount();
  });

  it("forwards id and class to the trigger, not the renderless root", async () => {
    // A FormField's `for=` must reach the button, and a caller's class must actually size it.
    const w = mountMenu({ size: "md" }, { id: "reg-kind", class: "max-w-xs" });
    const trigger = w.get("[aria-label]");
    expect(trigger.attributes("id")).toBe("reg-kind");
    expect(trigger.classes()).toContain("max-w-xs");
    // Reka labels the popover by the trigger's id — it must follow the caller's id, not dangle.
    const body = await open(w);
    expect(body.find('[aria-labelledby="reg-kind"]').exists()).toBe(true);
    w.unmount();
  });

  it("still labels the popover by the trigger when no id is given", async () => {
    const w = mountMenu();
    const id = w.get("[aria-label]").attributes("id");
    expect(id).toBeTruthy();
    const body = await open(w);
    expect(body.find(`[aria-labelledby="${id}"]`).exists()).toBe(true);
    w.unmount();
  });
});
