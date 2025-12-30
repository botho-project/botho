import { useState, useEffect } from 'react'
import type { Balance } from '@botho/core'
import { formatBTH, parseBTH } from '@botho/core'
import { Button, Input } from '@botho/ui'
import { motion, AnimatePresence } from 'motion/react'
import { Eye, Loader2, Send, Shield, X, Zap } from 'lucide-react'

export type SendPrivacyLevel = 'standard' | 'private'

export interface SendFormData {
  recipient: string
  amount: bigint
  privacyLevel: SendPrivacyLevel
  memo?: string
}

export interface SendResult {
  success: boolean
  txHash?: string
  error?: string
}

export interface SendModalProps {
  /** Whether the modal is open */
  isOpen: boolean
  /** Close handler */
  onClose: () => void
  /** Current balance (for max button) */
  balance: Balance | null
  /** Fee estimator function */
  estimateFee: (amount: bigint, privacyLevel: SendPrivacyLevel) => Promise<bigint>
  /** Send handler */
  onSend: (data: SendFormData) => Promise<SendResult>
  /** Whether a send is in progress */
  isSending?: boolean
}

/**
 * Modal for sending BTH transactions.
 */
export function SendModal({
  isOpen,
  onClose,
  balance,
  estimateFee,
  onSend,
  isSending = false,
}: SendModalProps) {
  const [recipient, setRecipient] = useState('')
  const [amount, setAmount] = useState('')
  const [privacyLevel, setPrivacyLevel] = useState<SendPrivacyLevel>('standard')
  const [memo, setMemo] = useState('')
  const [fee, setFee] = useState<bigint>(BigInt(0))
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  // Reset form when modal closes
  useEffect(() => {
    if (!isOpen) {
      setRecipient('')
      setAmount('')
      setMemo('')
      setError(null)
      setSuccess(null)
    }
  }, [isOpen])

  // Estimate fee when amount or privacy changes
  useEffect(() => {
    if (amount) {
      try {
        const amountBigInt = parseBTH(amount)
        estimateFee(amountBigInt, privacyLevel).then(setFee)
      } catch {
        // Invalid amount
      }
    }
  }, [amount, privacyLevel, estimateFee])

  const handleSend = async () => {
    setError(null)
    setSuccess(null)

    if (!recipient) {
      setError('Please enter a recipient address')
      return
    }

    if (!amount) {
      setError('Please enter an amount')
      return
    }

    try {
      const amountBigInt = parseBTH(amount)
      const total = amountBigInt + fee

      if (balance && total > balance.available) {
        setError('Insufficient balance')
        return
      }

      const result = await onSend({
        recipient,
        amount: amountBigInt,
        privacyLevel,
        memo: memo || undefined,
      })

      if (result.success) {
        setSuccess(`Transaction sent! Hash: ${result.txHash}`)
        setTimeout(() => {
          onClose()
        }, 2000)
      } else {
        setError(result.error || 'Transaction failed')
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Invalid amount')
    }
  }

  if (!isOpen) return null

  return (
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
        onClick={onClose}
      >
        <motion.div
          initial={{ opacity: 0, scale: 0.95, y: 20 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: 20 }}
          onClick={(e) => e.stopPropagation()}
          className="relative w-full max-w-md rounded-2xl border border-[--color-steel] bg-[--color-abyss] p-6 shadow-2xl"
        >
          {/* Close button */}
          <button
            onClick={onClose}
            className="absolute right-4 top-4 rounded-lg p-1 text-[--color-dim] transition-colors hover:bg-[--color-steel] hover:text-[--color-light]"
          >
            <X className="h-5 w-5" />
          </button>

          {/* Header */}
          <div className="mb-6 flex items-center gap-3">
            <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-[--color-pulse]/20">
              <Send className="h-5 w-5 text-[--color-pulse]" />
            </div>
            <div>
              <h2 className="font-display text-lg font-bold text-[--color-light]">Send BTH</h2>
              <p className="text-sm text-[--color-dim]">Transfer funds securely</p>
            </div>
          </div>

          {/* Form */}
          <div className="space-y-4">
            {/* Recipient */}
            <div>
              <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                Recipient Address
              </label>
              <Input
                placeholder="bth1..."
                value={recipient}
                onChange={(e) => setRecipient(e.target.value)}
                className="font-mono text-sm"
              />
            </div>

            {/* Amount */}
            <div>
              <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                Amount
              </label>
              <div className="relative">
                <Input
                  type="text"
                  placeholder="0.00"
                  value={amount}
                  onChange={(e) => setAmount(e.target.value)}
                  className="pr-16 font-mono"
                />
                <span className="absolute right-4 top-1/2 -translate-y-1/2 text-sm font-medium text-[--color-dim]">
                  BTH
                </span>
              </div>
              {balance && (
                <button
                  onClick={() => setAmount(formatBTH(balance.available, { separators: false }))}
                  className="mt-1 text-xs text-[--color-pulse] hover:underline"
                >
                  Max: {formatBTH(balance.available)} BTH
                </button>
              )}
            </div>

            {/* Privacy Level */}
            <div>
              <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                Privacy Level
              </label>
              <div className="grid grid-cols-2 gap-2">
                <button
                  onClick={() => setPrivacyLevel('standard')}
                  className={`flex items-center justify-center gap-2 rounded-lg border p-3 transition-all ${
                    privacyLevel === 'standard'
                      ? 'border-[--color-pulse] bg-[--color-pulse]/10 text-[--color-pulse]'
                      : 'border-[--color-steel] bg-[--color-slate] text-[--color-ghost] hover:border-[--color-pulse]/50'
                  }`}
                >
                  <Zap className="h-4 w-4" />
                  <span className="text-sm font-medium">Standard</span>
                </button>
                <button
                  onClick={() => setPrivacyLevel('private')}
                  className={`flex items-center justify-center gap-2 rounded-lg border p-3 transition-all ${
                    privacyLevel === 'private'
                      ? 'border-[--color-purple] bg-[--color-purple]/10 text-[--color-purple]'
                      : 'border-[--color-steel] bg-[--color-slate] text-[--color-ghost] hover:border-[--color-purple]/50'
                  }`}
                >
                  <Shield className="h-4 w-4" />
                  <span className="text-sm font-medium">Private</span>
                </button>
              </div>
              <p className="mt-1.5 text-xs text-[--color-dim]">
                {privacyLevel === 'standard'
                  ? 'Hidden amounts, visible sender. Lower fees (~3-4 KB).'
                  : 'Hidden amounts + sender (LION ring signature). Higher fees (~22 KB).'}
              </p>
            </div>

            {/* Memo */}
            <div>
              <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                Memo <span className="text-[--color-dim]">(optional)</span>
              </label>
              <Input
                placeholder="Add a note..."
                value={memo}
                onChange={(e) => setMemo(e.target.value)}
              />
            </div>

            {/* Fee Summary */}
            <div className="rounded-lg border border-[--color-steel] bg-[--color-slate]/50 p-3">
              <div className="flex items-center justify-between text-sm">
                <span className="text-[--color-ghost]">Network Fee</span>
                <span className="font-mono text-[--color-light]">{formatBTH(fee)} BTH</span>
              </div>
              {amount && (
                <div className="mt-2 flex items-center justify-between border-t border-[--color-steel] pt-2 text-sm">
                  <span className="font-medium text-[--color-ghost]">Total</span>
                  <span className="font-mono font-semibold text-[--color-pulse]">
                    {formatBTH(parseBTH(amount || '0') + fee)} BTH
                  </span>
                </div>
              )}
            </div>

            {/* Error/Success */}
            {error && (
              <div className="rounded-lg border border-[--color-danger]/30 bg-[--color-danger]/10 p-3 text-sm text-[--color-danger]">
                {error}
              </div>
            )}
            {success && (
              <div className="rounded-lg border border-[--color-success]/30 bg-[--color-success]/10 p-3 text-sm text-[--color-success]">
                {success}
              </div>
            )}

            {/* Submit */}
            <Button
              onClick={handleSend}
              disabled={isSending || !recipient || !amount}
              className="w-full"
            >
              {isSending ? (
                <>
                  <Loader2 className="h-4 w-4 animate-spin" />
                  Sending...
                </>
              ) : (
                <>
                  <Zap className="h-4 w-4" />
                  Send Transaction
                </>
              )}
            </Button>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  )
}
