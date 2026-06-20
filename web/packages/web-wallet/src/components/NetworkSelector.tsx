import { useState, useRef, useEffect } from 'react'
import { Button, Input } from '@botho/ui'
import { ChevronDown, Server, Check, Loader2, AlertCircle, X } from 'lucide-react'
import { useNetwork } from '../contexts/network'
import type { NodeHealth } from '../config/networks'

interface NetworkSelectorProps {
  /** Additional CSS classes */
  className?: string
}

/** Small colored dot + label describing a node's health. */
function HealthDot({ health }: { health: NodeHealth | undefined }) {
  const status = health?.status ?? 'checking'
  const color =
    status === 'online' ? 'bg-success' : status === 'offline' ? 'bg-danger' : 'bg-ghost'
  return <div className={`w-2 h-2 rounded-full shrink-0 ${color}`} />
}

/** One-line health summary text for a node. */
function healthLabel(health: NodeHealth | undefined): string {
  if (!health || health.status === 'checking') return 'Checking…'
  if (health.status === 'offline') return 'Unreachable'
  const h = health.chainHeight != null ? `height ${health.chainHeight}` : 'online'
  const sync = health.synced ? 'synced' : `${Math.round(health.syncProgress ?? 0)}%`
  return `${h} · ${sync}`
}

export function NetworkSelector({ className = '' }: NetworkSelectorProps) {
  const {
    network,
    ingressId,
    ingressNodes,
    nodeHealth,
    selectIngress,
    setCustomEndpoint,
    isValidating,
    validationError,
  } = useNetwork()
  const [isOpen, setIsOpen] = useState(false)
  const [showCustomInput, setShowCustomInput] = useState(false)
  const [customEndpoint, setCustomEndpointInput] = useState('')
  const dropdownRef = useRef<HTMLDivElement>(null)

  // Close dropdown when clicking outside
  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
        setIsOpen(false)
        setShowCustomInput(false)
      }
    }

    document.addEventListener('mousedown', handleClickOutside)
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [])

  const handleSelect = (nodeId: string) => {
    selectIngress(nodeId)
    setIsOpen(false)
    setShowCustomInput(false)
  }

  const handleCustomSubmit = async () => {
    if (!customEndpoint.trim()) return

    const success = await setCustomEndpoint(customEndpoint.trim())
    if (success) {
      setIsOpen(false)
      setShowCustomInput(false)
      setCustomEndpointInput('')
    }
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      handleCustomSubmit()
    } else if (e.key === 'Escape') {
      setShowCustomInput(false)
      setCustomEndpointInput('')
    }
  }

  // Label shown on the trigger button: the selected ingress node's name.
  const selectedNode = ingressNodes.find((n) => n.id === ingressId)
  const triggerLabel = selectedNode?.name ?? (ingressId === 'custom' ? 'Custom RPC' : 'Node')

  return (
    <div className={`relative ${className}`} ref={dropdownRef}>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => setIsOpen(!isOpen)}
        className="gap-2"
      >
        <Server size={14} />
        <span className="hidden sm:inline max-w-[10rem] truncate">{triggerLabel}</span>
        {network.isTestnet && (
          <span className="px-1.5 py-0.5 text-xs rounded bg-warning/20 text-warning">
            Testnet
          </span>
        )}
        <ChevronDown size={14} className={`transition-transform ${isOpen ? 'rotate-180' : ''}`} />
      </Button>

      {isOpen && (
        <div className="absolute right-0 mt-2 w-72 rounded-lg bg-abyss border border-steel shadow-lg z-50 overflow-hidden">
          <div className="p-2 border-b border-steel">
            <span className="text-xs text-ghost uppercase tracking-wide px-2">
              Trusted RPC ingress
            </span>
          </div>

          <div className="py-1">
            {ingressNodes.map((node) => {
              const health = nodeHealth[node.id]
              const selected = ingressId === node.id
              return (
                <button
                  key={node.id}
                  onClick={() => handleSelect(node.id)}
                  className={`w-full px-3 py-2 flex items-center gap-3 hover:bg-steel/50 transition-colors ${
                    selected ? 'bg-steel/30' : ''
                  }`}
                >
                  <HealthDot health={health} />
                  <div className="flex-1 text-left min-w-0">
                    <div className="text-sm text-light flex items-center gap-2">
                      {node.name}
                      {node.servesFaucet && (
                        <span className="px-1 py-0.5 text-[10px] rounded bg-pulse/15 text-pulse">
                          faucet
                        </span>
                      )}
                    </div>
                    <div className="text-xs text-ghost truncate">{healthLabel(health)}</div>
                  </div>
                  {selected && <Check size={16} className="text-pulse shrink-0" />}
                </button>
              )
            })}

            {/* Custom endpoint option */}
            <button
              onClick={() => setShowCustomInput((v) => !v)}
              className={`w-full px-3 py-2 flex items-center gap-3 hover:bg-steel/50 transition-colors ${
                ingressId === 'custom' ? 'bg-steel/30' : ''
              }`}
            >
              <div className="w-2 h-2 rounded-full bg-ghost shrink-0" />
              <div className="flex-1 text-left min-w-0">
                <div className="text-sm text-light">Custom RPC</div>
                {ingressId === 'custom' && (
                  <div className="text-xs text-ghost truncate">{network.rpcEndpoint}</div>
                )}
              </div>
              {ingressId === 'custom' && <Check size={16} className="text-pulse shrink-0" />}
            </button>
          </div>

          {/* Custom endpoint input */}
          {showCustomInput && (
            <div className="p-3 border-t border-steel">
              <div className="flex gap-2">
                <Input
                  type="url"
                  placeholder="https://node.example.com/rpc"
                  value={customEndpoint}
                  onChange={(e: React.ChangeEvent<HTMLInputElement>) => setCustomEndpointInput(e.target.value)}
                  onKeyDown={handleKeyDown}
                  className="flex-1 text-sm"
                  autoFocus
                />
                <Button
                  size="sm"
                  onClick={handleCustomSubmit}
                  disabled={!customEndpoint.trim() || isValidating}
                >
                  {isValidating ? <Loader2 size={14} className="animate-spin" /> : 'Connect'}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setShowCustomInput(false)
                    setCustomEndpointInput('')
                  }}
                >
                  <X size={14} />
                </Button>
              </div>
              {validationError && (
                <div className="flex items-center gap-2 mt-2 text-xs text-danger">
                  <AlertCircle size={12} />
                  {validationError}
                </div>
              )}
            </div>
          )}

          <div className="px-3 py-2 border-t border-steel">
            <p className="text-[11px] text-ghost leading-snug">
              Keys never leave your device. Faucet requests always use the faucet node.
            </p>
          </div>
        </div>
      )}
    </div>
  )
}
