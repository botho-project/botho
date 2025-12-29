import { cn } from '@/lib/utils'
import { motion } from 'motion/react'
import type { LucideIcon } from 'lucide-react'

interface StatsCardProps {
  title: string
  value: string | number
  change?: {
    value: number
    label: string
  }
  icon: LucideIcon
  color?: 'pulse' | 'success' | 'warning' | 'purple'
  delay?: number
}

const colorMap = {
  pulse: {
    bg: 'bg-[--color-pulse]/10',
    text: 'text-[--color-pulse]',
    glow: 'shadow-[0_0_20px_rgba(6,182,212,0.2)]',
  },
  success: {
    bg: 'bg-[--color-success]/10',
    text: 'text-[--color-success]',
    glow: 'shadow-[0_0_20px_rgba(16,185,129,0.2)]',
  },
  warning: {
    bg: 'bg-[--color-warning]/10',
    text: 'text-[--color-warning]',
    glow: 'shadow-[0_0_20px_rgba(245,158,11,0.2)]',
  },
  purple: {
    bg: 'bg-[--color-purple]/10',
    text: 'text-[--color-purple]',
    glow: 'shadow-[0_0_20px_rgba(139,92,246,0.2)]',
  },
}

export function StatsCard({ title, value, change, icon: Icon, color = 'pulse', delay = 0 }: StatsCardProps) {
  const colors = colorMap[color]

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.5, delay }}
      className={cn(
        'group relative overflow-hidden rounded-xl border border-[--color-steel] bg-[--color-abyss]/80 p-5 backdrop-blur-sm transition-all duration-300 hover:border-[--color-pulse-dim]',
        colors.glow
      )}
    >
      {/* Background gradient on hover */}
      <div className="absolute inset-0 bg-gradient-to-br from-[--color-pulse]/5 to-transparent opacity-0 transition-opacity group-hover:opacity-100" />

      <div className="relative flex items-start justify-between">
        <div>
          <p className="font-display text-xs font-semibold uppercase tracking-wider text-[--color-dim]">
            {title}
          </p>
          <motion.p
            initial={{ opacity: 0, scale: 0.5 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={{ duration: 0.5, delay: delay + 0.2 }}
            className="mt-2 font-display text-3xl font-bold text-[--color-light]"
          >
            {typeof value === 'number' ? value.toLocaleString() : value}
          </motion.p>
          {change && (
            <p className={cn('mt-2 text-xs', change.value >= 0 ? 'text-[--color-success]' : 'text-[--color-danger]')}>
              {change.value >= 0 ? '+' : ''}
              {change.value}% {change.label}
            </p>
          )}
        </div>
        <div className={cn('rounded-lg p-3', colors.bg)}>
          <Icon className={cn('h-6 w-6', colors.text)} />
        </div>
      </div>
    </motion.div>
  )
}
