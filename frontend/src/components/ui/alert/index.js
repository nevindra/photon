import { cva } from 'class-variance-authority'

export { default as Alert } from './Alert.vue'
export { default as AlertTitle } from './AlertTitle.vue'
export { default as AlertDescription } from './AlertDescription.vue'

export const alertVariants = cva(
  'relative w-full rounded-lg border px-4 py-3 text-sm [&>svg]:absolute [&>svg]:left-4 [&>svg]:top-4 [&>svg]:h-4 [&>svg]:w-4 [&>svg~*]:pl-7',
  {
    variants: {
      variant: {
        default: 'border-border bg-card text-card-foreground',
        success: 'border-success/30 bg-success-soft text-success',
        error: 'border-sev-error/30 bg-sev-error-soft text-sev-error',
        warning: 'border-sev-warn/30 bg-sev-warn-soft text-sev-warn',
        info: 'border-border bg-muted text-foreground',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  },
)
