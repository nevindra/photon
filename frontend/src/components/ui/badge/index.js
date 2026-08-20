import { cva } from 'class-variance-authority'

export { default as Badge } from './Badge.vue'

/* Badges are pills: rounded-full, 11px, medium weight, one line tall. Two families —
   SOLID (`default`/`destructive`, for counts and hard states) and SOFT (`success`/
   `warning`/`error`/`brand`, a tinted background with the matching foreground, held by an
   inset ring so the tint has an edge on white). `outline` is the neutral entity chip
   (a service, a zone, a host) — card-coloured with a hairline.
   Severity soft tints stay reserved for warn/error/fatal, per the colour policy. */
export const badgeVariants = cva(
  'inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium leading-4 transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 [&_svg]:size-3 [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        default: 'bg-primary text-primary-foreground',
        secondary: 'bg-secondary text-secondary-foreground',
        destructive: 'bg-destructive text-destructive-foreground',
        outline:
          'border border-border bg-card text-muted-foreground',
        success: 'bg-success-soft text-success ring-1 ring-inset ring-success/15',
        warning: 'bg-sev-warn-soft text-sev-warn ring-1 ring-inset ring-sev-warn/15',
        error: 'bg-sev-error-soft text-sev-error ring-1 ring-inset ring-sev-error/15',
        brand: 'bg-brand-soft text-brand ring-1 ring-inset ring-brand/15',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  },
)
