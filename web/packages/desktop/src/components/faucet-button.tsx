/**
 * FaucetButton component for requesting testnet BTH.
 *
 * Only visible when connected to a testnet with faucet support.
 * Handles loading states, rate limiting errors, and success notifications.
 */

import { useState, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Button } from '@botho/ui'
import { motion, AnimatePresence } from 'motion/react'
import { Droplets, Check, AlertCircle, Loader2 } from 'lucide-react'

/** Result of a faucet request from Tauri backend */
interface FaucetRequestResult {
  success: boolean
  txHash?: string
  amount?: string
  amountFormatted?: string
  error?: string
  retryAfterSecs?: number
}

interface FaucetButtonProps {
  /** Faucet server host */
  faucetHost: string
  /** Faucet server port */
  faucetPort: number
  /** Called when faucet request succeeds (to refresh balance) */
  onSuccess?: () => void
  /** Whether the wallet is unlocked (required for faucet requests) */
  isUnlocked: boolean
  /** Called when unlock is needed */
  onUnlockRequired?: () => void
}

type FaucetStatus = 'idle' | 'loading' | 'success' | 'error'

export function FaucetButton({
  faucetHost,
  faucetPort,
  onSuccess,
  isUnlocked,
  onUnlockRequired,
}: FaucetButtonProps) {
  const [status, setStatus] = useState<FaucetStatus>('idle')
  const [message, setMessage] = useState<string | null>(null)
  const [retryAfter, setRetryAfter] = useState<number | null>(null)

  const handleRequest = useCallback(async () => {
    // Require wallet unlock for faucet requests
    if (!isUnlocked) {
      onUnlockRequired?.()
      return
    }

    setStatus('loading')
    setMessage(null)
    setRetryAfter(null)

    try {
      const result = await invoke<FaucetRequestResult>('request_faucet', {
        params: {
          faucetHost,
          faucetPort,
        },
      })

      if (result.success) {
        setStatus('success')
        setMessage(`Received ${result.amountFormatted}`)
        onSuccess?.()

        // Reset to idle after 5 seconds
        setTimeout(() => {
          setStatus('idle')
          setMessage(null)
        }, 5000)
      } else {
        setStatus('error')
        setMessage(result.error || 'Faucet request failed')

        if (result.retryAfterSecs) {
          setRetryAfter(result.retryAfterSecs)
        }

        // Reset to idle after 10 seconds
        setTimeout(() => {
          setStatus('idle')
          setMessage(null)
          setRetryAfter(null)
        }, 10000)
      }
    } catch (err) {
      setStatus('error')
      setMessage(err instanceof Error ? err.message : 'Faucet request failed')

      // Reset to idle after 10 seconds
      setTimeout(() => {
        setStatus('idle')
        setMessage(null)
      }, 10000)
    }
  }, [faucetHost, faucetPort, isUnlocked, onSuccess, onUnlockRequired])

  // Countdown timer for rate limiting
  const formatRetryTime = (secs: number): string => {
    if (secs >= 3600) {
      const hours = Math.floor(secs / 3600)
      return `${hours}h ${Math.floor((secs % 3600) / 60)}m`
    }
    if (secs >= 60) {
      return `${Math.floor(secs / 60)}m ${secs % 60}s`
    }
    return `${secs}s`
  }

  return (
    <div className="relative">
      <Button
        variant="secondary"
        onClick={handleRequest}
        disabled={status === 'loading' || !isUnlocked}
        className="gap-2"
        title={isUnlocked ? 'Request testnet BTH' : 'Unlock wallet to request testnet BTH'}
      >
        {status === 'loading' ? (
          <Loader2 className="h-4 w-4 animate-spin" />
        ) : status === 'success' ? (
          <Check className="h-4 w-4 text-[--color-success]" />
        ) : status === 'error' ? (
          <AlertCircle className="h-4 w-4 text-[--color-danger]" />
        ) : (
          <Droplets className="h-4 w-4" />
        )}
        {status === 'loading' ? 'Requesting...' : 'Faucet'}
      </Button>

      {/* Status message popup */}
      <AnimatePresence>
        {message && (
          <motion.div
            initial={{ opacity: 0, y: -10, scale: 0.95 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -10, scale: 0.95 }}
            className={`absolute top-full left-1/2 z-50 mt-2 -translate-x-1/2 whitespace-nowrap rounded-lg px-3 py-2 text-sm shadow-lg ${
              status === 'success'
                ? 'border border-[--color-success]/30 bg-[--color-success]/10 text-[--color-success]'
                : 'border border-[--color-danger]/30 bg-[--color-danger]/10 text-[--color-danger]'
            }`}
          >
            <div className="flex items-center gap-2">
              {status === 'success' ? (
                <Check className="h-4 w-4" />
              ) : (
                <AlertCircle className="h-4 w-4" />
              )}
              <span>{message}</span>
            </div>
            {retryAfter !== null && retryAfter > 0 && (
              <div className="mt-1 text-xs opacity-80">
                Retry in {formatRetryTime(retryAfter)}
              </div>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}
