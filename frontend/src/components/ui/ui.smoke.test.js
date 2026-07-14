// Compile + smoke test for the shadcn-vue primitives. Importing each .vue forces
// Vitest (via @vitejs/plugin-vue) to compile it — this catches SFC/type-resolution
// errors even though no app code imports these yet. A few standalone primitives are
// mounted to confirm they render; composite/portal primitives (Sheet/Tooltip/
// DropdownMenu content) are import-only since they require their Root context.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'

import { Button, buttonVariants } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge, badgeVariants } from '@/components/ui/badge'
import { Separator } from '@/components/ui/separator'
import { Checkbox } from '@/components/ui/checkbox'
import { Switch } from '@/components/ui/switch'
import { ScrollArea, ScrollBar } from '@/components/ui/scroll-area'
import { ToggleGroup, ToggleGroupItem, toggleVariants } from '@/components/ui/toggle-group'
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from '@/components/ui/tooltip'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuCheckboxItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
} from '@/components/ui/dropdown-menu'
import {
  Sheet,
  SheetTrigger,
  SheetClose,
  SheetContent,
  SheetHeader,
  SheetFooter,
  SheetTitle,
  SheetDescription,
  sheetVariants,
} from '@/components/ui/sheet'
import {
  Popover,
  PopoverTrigger,
  PopoverContent,
  PopoverAnchor,
  PopoverClose,
} from '@/components/ui/popover'
import {
  Dialog,
  DialogTrigger,
  DialogClose,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
  CardFooter,
} from '@/components/ui/card'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { Kbd } from '@/components/ui/kbd'
import { StatusDot } from '@/components/ui/status-dot'
import { StatusPill } from '@/components/ui/status-pill'
import { StatTile } from '@/components/ui/stat-tile'
import { Alert, AlertTitle, AlertDescription, alertVariants } from '@/components/ui/alert'
import { FormField } from '@/components/ui/form-field'
import { NumberField } from '@/components/ui/number-field'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import {
  Table,
  TableHeader,
  TableBody,
  TableFooter,
  TableRow,
  TableHead,
  TableCell,
  TableCaption,
} from '@/components/ui/table'
import {
  Select,
  SelectTrigger,
  SelectContent,
  SelectItem,
  SelectValue,
  SelectGroup,
  SelectLabel,
  SelectSeparator,
} from '@/components/ui/select'
import { Toaster, useToast, toast, toastVariants } from '@/components/ui/toast'

describe('ui primitives — import & compile', () => {
  it('every primitive is a defined component', () => {
    const components = [
      Button, Input, Label, Badge, Separator, Checkbox, Switch, ScrollArea, ScrollBar,
      ToggleGroup, ToggleGroupItem,
      Tooltip, TooltipTrigger, TooltipContent, TooltipProvider,
      DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuGroup,
      DropdownMenuItem, DropdownMenuCheckboxItem, DropdownMenuRadioGroup,
      DropdownMenuRadioItem, DropdownMenuLabel, DropdownMenuSeparator,
      DropdownMenuShortcut, DropdownMenuSub, DropdownMenuSubTrigger, DropdownMenuSubContent,
      Sheet, SheetTrigger, SheetClose, SheetContent, SheetHeader, SheetFooter,
      SheetTitle, SheetDescription,
      Popover, PopoverTrigger, PopoverContent, PopoverAnchor, PopoverClose,
      Dialog, DialogTrigger, DialogClose, DialogContent, DialogHeader, DialogFooter,
      DialogTitle, DialogDescription,
    ]
    for (const c of components) expect(c).toBeTruthy()
  })

  it('exposes cva variant helpers', () => {
    expect(typeof buttonVariants).toBe('function')
    expect(typeof badgeVariants).toBe('function')
    expect(typeof toggleVariants).toBe('function')
    expect(typeof sheetVariants).toBe('function')
    expect(sheetVariants({ side: 'right' })).toContain('inset-y-0')
  })
})

describe('ui primitives — render', () => {
  it('Button renders slot content with the selected variant', () => {
    const w = mount(Button, { props: { variant: 'outline' }, slots: { default: 'Go' } })
    expect(w.text()).toContain('Go')
    expect(w.classes()).toContain('border')
  })

  it('Badge renders with default (primary) variant', () => {
    const w = mount(Badge, { slots: { default: 'new' } })
    expect(w.text()).toBe('new')
    expect(w.classes()).toContain('bg-primary')
  })

  it('Separator renders a border-coloured divider', () => {
    const w = mount(Separator)
    expect(w.html()).toContain('bg-border')
  })

  it('Input reflects modelValue and emits update on input', async () => {
    const w = mount(Input, { props: { modelValue: 'hi' } })
    expect(w.element.value).toBe('hi')
    await w.setValue('there')
    expect(w.emitted('update:modelValue')?.[0]).toEqual(['there'])
  })

  it('Switch and Checkbox mount as interactive controls', () => {
    expect(mount(Switch).exists()).toBe(true)
    expect(mount(Checkbox).exists()).toBe(true)
  })

  it('ScrollArea renders its slotted content', () => {
    const w = mount(ScrollArea, { slots: { default: 'body' } })
    expect(w.text()).toContain('body')
  })
})

describe('new ui primitives — import & compile', () => {
  it('every new primitive is a defined component', () => {
    const components = [
      Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter,
      EmptyState, Skeleton, Spinner, Kbd,
      StatusDot, StatusPill, StatTile,
      Alert, AlertTitle, AlertDescription,
      FormField, NumberField,
      Segmented, SegmentedItem,
      Table, TableHeader, TableBody, TableFooter, TableRow, TableHead, TableCell, TableCaption,
      Select, SelectTrigger, SelectContent, SelectItem, SelectValue, SelectGroup,
      SelectLabel, SelectSeparator,
      Toaster,
    ]
    for (const c of components) expect(c).toBeTruthy()
  })

  it('exposes new cva + toast helpers', () => {
    expect(typeof alertVariants).toBe('function')
    expect(typeof toastVariants).toBe('function')
    expect(typeof toast).toBe('function')
    expect(typeof useToast).toBe('function')
    expect(alertVariants({ variant: 'error' })).toContain('sev-error')
  })
})

describe('new ui primitives — render', () => {
  it('Card renders slotted content on a card surface', () => {
    const w = mount(Card, { slots: { default: 'body' } })
    expect(w.text()).toContain('body')
    expect(w.classes()).toContain('bg-card')
  })

  it('EmptyState shows its title and description', () => {
    const w = mount(EmptyState, { props: { title: 'No logs match', description: 'Widen the range' } })
    expect(w.text()).toContain('No logs match')
    expect(w.text()).toContain('Widen the range')
  })

  it('Skeleton renders a pulsing placeholder', () => {
    expect(mount(Skeleton).classes()).toContain('animate-pulse')
  })

  it('Spinner renders a spinning icon', () => {
    expect(mount(Spinner).html()).toContain('animate-spin')
  })

  it('Kbd renders a key', () => {
    expect(mount(Kbd, { slots: { default: '⌘K' } }).text()).toContain('⌘K')
  })

  it('StatusDot applies its tone colour', () => {
    expect(mount(StatusDot, { props: { tone: 'error' } }).html()).toContain('bg-sev-error')
  })

  it('StatusPill renders label + success tone', () => {
    const w = mount(StatusPill, { props: { tone: 'success' }, slots: { default: 'Up' } })
    expect(w.text()).toContain('Up')
    expect(w.html()).toContain('text-success')
  })

  it('StatTile shows value and label', () => {
    const w = mount(StatTile, { props: { label: 'Uptime', value: '99.9%' } })
    expect(w.text()).toContain('99.9%')
    expect(w.text()).toContain('Uptime')
  })

  it('Alert renders variant styling, role and content', () => {
    const w = mount(Alert, { props: { variant: 'warning' }, slots: { default: 'careful' } })
    expect(w.text()).toContain('careful')
    expect(w.html()).toContain('sev-warn')
    expect(w.attributes('role')).toBe('alert')
  })

  it('FormField shows hint, and error replaces hint', () => {
    const hint = mount(FormField, { props: { label: 'Name', hint: 'shown' } })
    expect(hint.text()).toContain('Name')
    expect(hint.text()).toContain('shown')
    const err = mount(FormField, { props: { label: 'Name', hint: 'shown', error: 'required' } })
    expect(err.text()).toContain('required')
    expect(err.text()).not.toContain('shown')
    expect(err.html()).toContain('text-sev-error')
  })
})
