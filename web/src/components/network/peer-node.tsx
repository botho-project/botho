import { memo } from 'react'
import { Handle, Position } from '@xyflow/react'
import { Shield, Radio, Server } from 'lucide-react'
import { cn } from '@/lib/utils'

export interface PeerNodeData extends Record<string, unknown> {
  label: string
  nodeId: string
  isValidator: boolean
  isSelf: boolean
  status: 'online' | 'syncing' | 'offline'
  latency?: number
  blockHeight?: number
  version?: string
}

interface PeerNodeProps {
  data: PeerNodeData
  selected?: boolean
}

function PeerNodeComponent({ data, selected }: PeerNodeProps) {
  const { label, isValidator, isSelf, status, latency } = data

  const statusColors: Record<string, string> = {
    online: 'bg-[--color-success]',
    syncing: 'bg-[--color-warning]',
    offline: 'bg-[--color-danger]',
  }

  return (
    <>
      <Handle type="target" position={Position.Top} className="!bg-[--color-steel] !border-0 !w-2 !h-2" />
      <div
        className={cn(
          'relative rounded-xl border bg-[--color-abyss]/95 px-4 py-3 backdrop-blur-sm transition-all',
          isSelf
            ? 'border-[--color-pulse] shadow-[0_0_20px_rgba(6,182,212,0.3)]'
            : selected
              ? 'border-[--color-purple]'
              : 'border-[--color-steel] hover:border-[--color-ghost]',
          isSelf && 'animate-pulse-slow'
        )}
      >
        {/* Status indicator */}
        <div className={cn('absolute -top-1 -right-1 h-3 w-3 rounded-full border-2 border-[--color-abyss]', statusColors[status])} />

        {/* Icon */}
        <div className="flex items-center gap-3">
          <div
            className={cn(
              'flex h-10 w-10 items-center justify-center rounded-lg',
              isSelf
                ? 'bg-[--color-pulse]/20'
                : isValidator
                  ? 'bg-[--color-purple]/20'
                  : 'bg-[--color-slate]'
            )}
          >
            {isSelf ? (
              <Radio className="h-5 w-5 text-[--color-pulse]" />
            ) : isValidator ? (
              <Shield className="h-5 w-5 text-[--color-purple]" />
            ) : (
              <Server className="h-5 w-5 text-[--color-dim]" />
            )}
          </div>

          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span
                className={cn(
                  'font-mono text-sm font-medium truncate max-w-[120px]',
                  isSelf ? 'text-[--color-pulse]' : 'text-[--color-light]'
                )}
              >
                {label}
              </span>
              {isSelf && (
                <span className="rounded bg-[--color-pulse]/20 px-1.5 py-0.5 text-[10px] font-medium text-[--color-pulse]">
                  YOU
                </span>
              )}
              {isValidator && !isSelf && (
                <span className="rounded bg-[--color-purple]/20 px-1.5 py-0.5 text-[10px] font-medium text-[--color-purple]">
                  VAL
                </span>
              )}
            </div>
            {latency !== undefined && latency > 0 && (
              <p className="text-xs text-[--color-dim]">{latency}ms</p>
            )}
          </div>
        </div>
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-[--color-steel] !border-0 !w-2 !h-2" />
    </>
  )
}

export const PeerNode = memo(PeerNodeComponent)
