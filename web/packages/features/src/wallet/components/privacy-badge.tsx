import type { CryptoType, PrivacyLevel } from '@botho/core'
import { Lock, Shield, Layers } from 'lucide-react'
import { useState } from 'react'

export interface PrivacyBadgeProps {
  /** Cryptographic signature type of the transaction */
  cryptoType: CryptoType
  /** Size variant */
  size?: 'sm' | 'md'
  /** Whether to show tooltip on hover */
  showTooltip?: boolean
  /** Custom class name */
  className?: string
}

/** @deprecated Use PrivacyBadgeProps with cryptoType instead */
export interface LegacyPrivacyBadgeProps {
  /** Privacy level of the transaction (legacy) */
  level: PrivacyLevel
  /** Size variant */
  size?: 'sm' | 'md'
  /** Custom class name */
  className?: string
}

const cryptoConfig = {
  clsag: {
    icon: Lock,
    label: 'Private',
    fullLabel: 'Private (CLSAG)',
    color: 'text-[#3B82F6]',
    bg: 'bg-[#3B82F6]/10',
    border: 'border-[#3B82F6]/30',
    tooltip: 'Uses CLSAG ring signatures to hide sender identity',
  },
  mldsa: {
    icon: Shield,
    label: 'Minting',
    fullLabel: 'Minting (ML-DSA)',
    color: 'text-[#8B5CF6]',
    bg: 'bg-[#8B5CF6]/10',
    border: 'border-[#8B5CF6]/30',
    tooltip: 'Uses ML-DSA post-quantum signatures for minting',
  },
  hybrid: {
    icon: Layers,
    label: 'Hybrid',
    fullLabel: 'Hybrid',
    color: 'text-[#6366F1]',
    bg: 'bg-gradient-to-r from-[#3B82F6]/10 to-[#8B5CF6]/10',
    border: 'border-[#6366F1]/30',
    tooltip: 'Uses both CLSAG and ML-DSA signatures',
  },
}

// Legacy config for backward compatibility
const legacyConfig = {
  standard: cryptoConfig.mldsa,
  private: cryptoConfig.clsag,
}

/**
 * Badge showing transaction cryptographic type with optional tooltip.
 */
export function PrivacyBadge({
  cryptoType,
  size = 'sm',
  showTooltip = true,
  className = '',
}: PrivacyBadgeProps) {
  const [isHovered, setIsHovered] = useState(false)
  const { icon: Icon, label, tooltip, color, bg, border } = cryptoConfig[cryptoType]

  const sizeClasses = size === 'sm' ? 'px-2 py-0.5 text-xs' : 'px-2.5 py-1 text-sm'
  const iconSize = size === 'sm' ? 'h-3 w-3' : 'h-4 w-4'

  return (
    <div className="relative inline-block">
      <div
        className={`inline-flex items-center gap-1 rounded-full border font-medium ${sizeClasses} ${bg} ${color} ${border} ${className}`}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
      >
        <Icon className={iconSize} />
        {label}
      </div>
      {showTooltip && isHovered && (
        <div className="absolute bottom-full left-1/2 z-50 mb-2 -translate-x-1/2 whitespace-nowrap rounded-md bg-[--color-void] px-3 py-2 text-xs text-[--color-light] shadow-lg border border-[--color-steel]">
          {tooltip}
          <div className="absolute left-1/2 top-full -translate-x-1/2 border-4 border-transparent border-t-[--color-void]" />
        </div>
      )}
    </div>
  )
}

/**
 * Legacy badge for backward compatibility with PrivacyLevel.
 * @deprecated Use PrivacyBadge with cryptoType instead
 */
export function LegacyPrivacyBadge({ level, size = 'sm', className = '' }: LegacyPrivacyBadgeProps) {
  const { icon: Icon, label, color, bg, border } = legacyConfig[level]

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
