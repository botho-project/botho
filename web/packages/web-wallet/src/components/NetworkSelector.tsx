import { useState, useRef, useEffect } from 'react'
import { Button, Input } from '@botho/ui'
import { ChevronDown, Globe, Check, Loader2, AlertCircle, X } from 'lucide-react'
import { useNetwork } from '../contexts/network'

interface NetworkSelectorProps {
  /** Additional CSS classes */
  className?: string
}

export function NetworkSelector({ className = '' }: NetworkSelectorProps) {
  const { network, availableNetworks, switchNetwork, setCustomEndpoint, isValidating, validationError } = useNetwork()
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

  const handleNetworkSelect = (networkId: string) => {
    if (networkId === 'custom') {
      setShowCustomInput(true)
    } else {
      switchNetwork(networkId)
      setIsOpen(false)
      setShowCustomInput(false)
    }
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

  return (
    <div className={`relative ${className}`} ref={dropdownRef}>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => setIsOpen(!isOpen)}
        className="gap-2"
      >
        <Globe size={14} />
        <span className="hidden sm:inline">{network.name}</span>
        {network.isTestnet && (
          <span className="px-1.5 py-0.5 text-xs rounded bg-warning/20 text-warning">
            Testnet
          </span>
        )}
        <ChevronDown size={14} className={`transition-transform ${isOpen ? 'rotate-180' : ''}`} />
      </Button>

      {isOpen && (
        <div className="absolute right-0 mt-2 w-64 rounded-lg bg-abyss border border-steel shadow-lg z-50 overflow-hidden">
          <div className="p-2 border-b border-steel">
            <span className="text-xs text-ghost uppercase tracking-wide px-2">Select Network</span>
          </div>

          <div className="py-1">
            {availableNetworks.map((net) => (
              <button
                key={net.id}
                onClick={() => handleNetworkSelect(net.id)}
                className={`w-full px-3 py-2 flex items-center gap-3 hover:bg-steel/50 transition-colors ${
                  network.id === net.id ? 'bg-steel/30' : ''
                }`}
              >
                <div className={`w-2 h-2 rounded-full ${net.isTestnet ? 'bg-warning' : 'bg-success'}`} />
                <div className="flex-1 text-left">
                  <div className="text-sm text-light">{net.name}</div>
                  <div className="text-xs text-ghost truncate">{net.rpcEndpoint}</div>
                </div>
                {network.id === net.id && <Check size={16} className="text-pulse" />}
              </button>
            ))}

            {/* Custom endpoint option */}
            <button
              onClick={() => handleNetworkSelect('custom')}
              className={`w-full px-3 py-2 flex items-center gap-3 hover:bg-steel/50 transition-colors ${
                network.id === 'custom' ? 'bg-steel/30' : ''
              }`}
            >
              <div className="w-2 h-2 rounded-full bg-ghost" />
              <div className="flex-1 text-left">
                <div className="text-sm text-light">Custom RPC</div>
                {network.id === 'custom' && (
                  <div className="text-xs text-ghost truncate">{network.rpcEndpoint}</div>
                )}
              </div>
              {network.id === 'custom' && <Check size={16} className="text-pulse" />}
            </button>
          </div>

          {/* Custom endpoint input */}
          {showCustomInput && (
            <div className="p-3 border-t border-steel">
              <div className="flex gap-2">
                <Input
                  type="url"
                  placeholder="https://node.example.com"
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
        </div>
      )}
    </div>
  )
}
