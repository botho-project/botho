import { cn } from '../lib/utils'
import type { HTMLAttributes } from 'react'

type ConnectionStatus = 'connected' | 'connecting' | 'disconnected' | 'reconnecting'

interface ConnectionIndicatorProps extends HTMLAttributes<HTMLDivElement> {
  status: ConnectionStatus
  showLabel?: boolean
}

const statusConfig: Record<ConnectionStatus, { color: string; label: string; pulse: boolean }> = {
  connected: {
    color: 'bg-green-500',
    label: 'Connected',
    pulse: false,
  },
  connecting: {
    color: 'bg-yellow-500',
    label: 'Connecting...',
    pulse: true,
  },
  disconnected: {
    color: 'bg-red-500',
    label: 'Disconnected',
    pulse: false,
  },
  reconnecting: {
    color: 'bg-yellow-500',
    label: 'Reconnecting...',
    pulse: true,
  },
}

export function ConnectionIndicator({
  status,
  showLabel = true,
  className,
  ...props
}: ConnectionIndicatorProps) {
  const config = statusConfig[status]

  return (
    <div
      className={cn('inline-flex items-center gap-2', className)}
      role="status"
      aria-label={`Connection status: ${config.label}`}
      {...props}
    >
      <span className="relative flex h-2.5 w-2.5">
        {config.pulse && (
          <span
            className={cn(
              'absolute inline-flex h-full w-full animate-ping rounded-full opacity-75',
              config.color
            )}
          />
        )}
        <span
          className={cn('relative inline-flex h-2.5 w-2.5 rounded-full', config.color)}
        />
      </span>
      {showLabel && (
        <span className="text-xs text-[--color-ghost]">{config.label}</span>
      )}
    </div>
  )
}
