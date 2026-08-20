import { cva } from 'class-variance-authority'

export { default as Button } from './Button.vue'

/* Buttons are flat. Solid variants carry `.pk` (a hairline ambient shadow + a 1px press,
   see styles/base.css); outline/ghost carry no shadow at all — an outline button is a
   white card-coloured surface held by a hairline, exactly like the panel it sits on.
   Radius comes from the shared scale: `rounded-md` (10px) for every size except `lg`. */
export const buttonVariants = cva(
  'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-sm font-medium ring-offset-background transition-[transform,box-shadow,background-color,border-color,color] duration-150 ease-out focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 disabled:translate-y-0 disabled:scale-100 disabled:shadow-none [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        default: 'pk bg-primary text-primary-foreground hover:bg-primary/85',
        destructive: 'pk bg-destructive text-destructive-foreground hover:bg-destructive/90',
        outline:
          'border border-border bg-surface-1 text-foreground shadow-1 hover:bg-muted hover:text-foreground',
        secondary: 'bg-secondary text-secondary-foreground hover:bg-secondary/70',
        brand: 'pk bg-brand text-brand-foreground hover:bg-brand-strong',
        ghost: 'text-muted-foreground hover:bg-muted hover:text-foreground',
        link: 'text-brand underline-offset-4 hover:underline',
      },
      size: {
        default: 'h-9 px-3.5 py-2',
        sm: 'h-8 px-3 text-xs',
        lg: 'h-10 rounded-lg px-6',
        icon: 'h-9 w-9',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
)
