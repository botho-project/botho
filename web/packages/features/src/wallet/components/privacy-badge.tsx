import type { PrivacyLevel } from '@botho/core'
import { Eye, ShieldCheck } from 'lucide-react'

export interface PrivacyBadgeProps {
  /** Privacy level of the transaction */
  level: PrivacyLevel
  /** Size variant */
  size?: 'sm' | 'md'
  /** Custom class name */
  className?: string
}

const config = {
  standard: {
    icon: Eye,
    label: 'Standard',
    color: 'text-[--color-ghost]',
    bg: 'bg-[--color-slate]/50',
    border: 'border-[--color-steel]',
  },
  private: {
    icon: ShieldCheck,
    label: 'Private',
    color: 'text-[--color-success]',
    bg: 'bg-[--color-success]/10',
    border: 'border-[--color-success]/30',
  },
}

/**
 * Badge showing transaction privacy level.
 */
export function PrivacyBadge({ level, size = 'sm', className = '' }: PrivacyBadgeProps) {
  const { icon: Icon, label, color, bg, border } = config[level]

  const sizeClasses = size === 'sm' ? 'px-2 py-0.5 text-xs' : 'px-2.5 py-1 text-sm'
  const iconSize = size === 'sm' ? 'h-3 w-3' : 'h-4 w-4'

  return (
    <div
      className={`inline-flex items-center gap-1 rounded-full border font-medium ${sizeClasses} ${bg} ${color} ${border} ${className}`}
    >
      <Icon className={iconSize} />
      {label}
    </div>
  )
}
