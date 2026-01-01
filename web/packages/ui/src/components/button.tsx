import { cn } from '../lib/utils'
import { cva, type VariantProps } from 'class-variance-authority'
import type { ButtonHTMLAttributes, ReactNode } from 'react'

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 rounded-xl font-display font-semibold transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[--color-pulse] disabled:pointer-events-none disabled:opacity-50',
  {
    variants: {
      variant: {
        primary: 'bg-[--color-pulse] text-[--color-void] hover:bg-[--color-pulse-dim] hover:shadow-[0_0_20px_rgba(6,182,212,0.3)]',
        secondary: 'border border-[--color-steel] bg-[--color-slate] text-[--color-light] hover:border-[--color-pulse-dim] hover:bg-[--color-steel]',
        ghost: 'text-[--color-ghost] hover:bg-[--color-steel] hover:text-[--color-light]',
        danger: 'bg-[--color-danger] text-white hover:bg-[--color-danger]/80',
      },
      size: {
        // Mobile: min 44px touch targets per WCAG 2.1 SC 2.5.5
        // Desktop (sm+): original compact sizes
        sm: 'min-h-[44px] min-w-[44px] px-3 text-xs sm:min-h-0 sm:min-w-0 sm:h-8',
        md: 'min-h-[44px] min-w-[44px] px-4 text-sm sm:min-h-0 sm:min-w-0 sm:h-10',
        lg: 'h-12 px-6 text-base',
        icon: 'min-h-[44px] min-w-[44px] sm:min-h-0 sm:min-w-0 sm:h-10 sm:w-10',
      },
    },
    defaultVariants: {
      variant: 'primary',
      size: 'md',
    },
  }
)

interface ButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  children: ReactNode
}

export function Button({ className, variant, size, children, ...props }: ButtonProps) {
  return (
    <button className={cn(buttonVariants({ variant, size }), className)} {...props}>
      {children}
    </button>
  )
}
