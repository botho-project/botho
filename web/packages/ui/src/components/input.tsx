import { cn } from '../lib/utils'
import type { InputHTMLAttributes } from 'react'
import { forwardRef } from 'react'

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  error?: string
}

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ className, type, error, ...props }, ref) => {
    return (
      <div className="w-full">
        <input
          type={type}
          className={cn(
            'h-10 w-full rounded-lg border bg-[--color-slate] px-4 text-sm text-[--color-light] placeholder:text-[--color-dim] focus:outline-none focus:ring-1',
            error
              ? 'border-[--color-danger] focus:border-[--color-danger] focus:ring-[--color-danger]'
              : 'border-[--color-steel] focus:border-[--color-pulse-dim] focus:ring-[--color-pulse-dim]',
            className
          )}
          ref={ref}
          {...props}
        />
        {error && <p className="mt-1 text-xs text-[--color-danger]">{error}</p>}
      </div>
    )
  }
)

Input.displayName = 'Input'
