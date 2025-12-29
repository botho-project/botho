import { cn } from '../lib/utils'

interface LogoProps {
  className?: string
  size?: 'sm' | 'md' | 'lg'
  showText?: boolean
}

const sizes = {
  sm: 'h-6 w-6',
  md: 'h-8 w-8',
  lg: 'h-12 w-12',
}

export function Logo({ className, size = 'md', showText = true }: LogoProps) {
  return (
    <div className={cn('flex items-center gap-3', className)}>
      <div className={cn('relative', sizes[size])}>
        <svg viewBox="0 0 32 32" className={sizes[size]}>
          <defs>
            <linearGradient id="logo-gradient" x1="0%" y1="0%" x2="100%" y2="100%">
              <stop offset="0%" stopColor="var(--color-pulse)" />
              <stop offset="100%" stopColor="var(--color-purple)" />
            </linearGradient>
          </defs>
          <circle cx="16" cy="16" r="14" fill="none" stroke="url(#logo-gradient)" strokeWidth="2" />
          <path
            d="M8 16 Q12 10, 16 16 T24 16"
            fill="none"
            stroke="url(#logo-gradient)"
            strokeWidth="2"
            strokeLinecap="round"
          >
            <animate
              attributeName="d"
              dur="2s"
              repeatCount="indefinite"
              values="M8 16 Q12 10, 16 16 T24 16;M8 16 Q12 22, 16 16 T24 16;M8 16 Q12 10, 16 16 T24 16"
            />
          </path>
        </svg>
      </div>
      {showText && (
        <div>
          <h1 className="font-display text-lg font-bold tracking-tight text-gradient">
            Botho
          </h1>
          <p className="text-xs text-[--color-dim]">Private Currency</p>
        </div>
      )}
    </div>
  )
}
