import { useState, useCallback, useEffect } from 'react'
import { Layout } from '../components/layout'
import { Card, CardHeader, CardTitle, CardContent, Button, Input } from '@botho/ui'
import { motion, AnimatePresence } from 'motion/react'
import {
  ArrowDownLeft,
  ArrowUpRight,
  Check,
  ChevronRight,
  Clock,
  Copy,
  Eye,
  EyeOff,
  Loader2,
  Lock,
  RefreshCw,
  Send,
  Shield,
  ShieldCheck,
  Sparkles,
  Wallet,
  X,
  Zap,
} from 'lucide-react'
import { useWallet, formatBTH, parseBTH } from '../contexts/wallet'
import { useConnection } from '../contexts/connection'
import type { Transaction, PrivacyLevel } from '@botho/core'

// Privacy level badge component
function PrivacyBadge({ level }: { level: PrivacyLevel }) {
  const config = {
    plain: {
      icon: Eye,
      label: 'Plain',
      color: 'text-[--color-warning]',
      bg: 'bg-[--color-warning]/10',
      border: 'border-[--color-warning]/30',
    },
    hidden: {
      icon: EyeOff,
      label: 'Hidden',
      color: 'text-[--color-purple]',
      bg: 'bg-[--color-purple]/10',
      border: 'border-[--color-purple]/30',
    },
    ring: {
      icon: ShieldCheck,
      label: 'Ring',
      color: 'text-[--color-success]',
      bg: 'bg-[--color-success]/10',
      border: 'border-[--color-success]/30',
    },
  }[level]

  const Icon = config.icon

  return (
    <div className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium border ${config.bg} ${config.color} ${config.border}`}>
      <Icon className="h-3 w-3" />
      {config.label}
    </div>
  )
}

// Transaction row component
function TransactionRow({ tx, index }: { tx: Transaction; index: number }) {
  const isReceive = tx.type === 'receive' || tx.type === 'minting'
  const Icon = tx.type === 'minting' ? Sparkles : isReceive ? ArrowDownLeft : ArrowUpRight

  const statusConfig = {
    pending: { color: 'text-[--color-warning]', icon: Clock },
    confirmed: { color: 'text-[--color-success]', icon: Check },
    failed: { color: 'text-[--color-danger]', icon: X },
  }[tx.status]

  const StatusIcon = statusConfig.icon

  return (
    <motion.div
      initial={{ opacity: 0, x: -20 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ delay: index * 0.05 }}
      className="group flex items-center gap-4 rounded-lg border border-transparent bg-[--color-slate]/50 p-4 transition-all hover:border-[--color-steel] hover:bg-[--color-slate]"
    >
      {/* Icon */}
      <div className={`flex h-10 w-10 items-center justify-center rounded-lg ${
        tx.type === 'minting'
          ? 'bg-[--color-purple]/20 text-[--color-purple]'
          : isReceive
            ? 'bg-[--color-success]/20 text-[--color-success]'
            : 'bg-[--color-danger]/20 text-[--color-danger]'
      }`}>
        <Icon className="h-5 w-5" />
      </div>

      {/* Details */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="font-display font-medium text-[--color-light]">
            {tx.type === 'minting' ? 'Minting Reward' : isReceive ? 'Received' : 'Sent'}
          </span>
          <PrivacyBadge level={tx.privacyLevel} />
        </div>
        <div className="flex items-center gap-2 mt-0.5">
          <span className="font-mono text-xs text-[--color-dim] truncate max-w-[180px]">
            {tx.counterparty || tx.id}
          </span>
          <span className="text-[--color-muted]">•</span>
          <span className="text-xs text-[--color-dim]">
            {new Date(tx.timestamp * 1000).toLocaleDateString()}
          </span>
        </div>
      </div>

      {/* Amount & Status */}
      <div className="text-right">
        <div className={`font-mono font-semibold ${isReceive ? 'text-[--color-success]' : 'text-[--color-light]'}`}>
          {isReceive ? '+' : '-'}{formatBTH(tx.amount)} BTH
        </div>
        <div className={`flex items-center justify-end gap-1 text-xs ${statusConfig.color}`}>
          <StatusIcon className="h-3 w-3" />
          {tx.status === 'confirmed' ? `${tx.confirmations} conf` : tx.status}
        </div>
      </div>

      {/* Chevron */}
      <ChevronRight className="h-4 w-4 text-[--color-dim] opacity-0 transition-opacity group-hover:opacity-100" />
    </motion.div>
  )
}

// Send modal component
function SendModal({
  isOpen,
  onClose,
}: {
  isOpen: boolean
  onClose: () => void
}) {
  const { sendTransaction, estimateFee, isSending, balance } = useWallet()
  const [recipient, setRecipient] = useState('')
  const [amount, setAmount] = useState('')
  const [privacyLevel, setPrivacyLevel] = useState<'plain' | 'hidden'>('plain')
  const [memo, setMemo] = useState('')
  const [fee, setFee] = useState<bigint>(BigInt(0))
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

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

      const result = await sendTransaction({
        recipient,
        amount: amountBigInt,
        privacyLevel,
        memo: memo || undefined,
      })

      if (result.success) {
        setSuccess(`Transaction sent! Hash: ${result.txHash}`)
        setTimeout(() => {
          onClose()
          setRecipient('')
          setAmount('')
          setMemo('')
          setSuccess(null)
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
                  onClick={() => setAmount(formatBTH(balance.available))}
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
                  onClick={() => setPrivacyLevel('plain')}
                  className={`flex items-center justify-center gap-2 rounded-lg border p-3 transition-all ${
                    privacyLevel === 'plain'
                      ? 'border-[--color-warning] bg-[--color-warning]/10 text-[--color-warning]'
                      : 'border-[--color-steel] bg-[--color-slate] text-[--color-ghost] hover:border-[--color-warning]/50'
                  }`}
                >
                  <Eye className="h-4 w-4" />
                  <span className="text-sm font-medium">Plain</span>
                </button>
                <button
                  onClick={() => setPrivacyLevel('hidden')}
                  className={`flex items-center justify-center gap-2 rounded-lg border p-3 transition-all ${
                    privacyLevel === 'hidden'
                      ? 'border-[--color-purple] bg-[--color-purple]/10 text-[--color-purple]'
                      : 'border-[--color-steel] bg-[--color-slate] text-[--color-ghost] hover:border-[--color-purple]/50'
                  }`}
                >
                  <Shield className="h-4 w-4" />
                  <span className="text-sm font-medium">Hidden</span>
                </button>
              </div>
              <p className="mt-1.5 text-xs text-[--color-dim]">
                {privacyLevel === 'plain'
                  ? 'Fast & cheap. Amount and addresses visible on-chain.'
                  : 'Private transaction. Uses ring signatures to hide sender.'}
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

// Address setup component
function AddressSetup({ onComplete }: { onComplete: (address: string) => void }) {
  const [address, setAddress] = useState('')
  const [error, setError] = useState<string | null>(null)

  const handleSubmit = () => {
    if (!address) {
      setError('Please enter your wallet address')
      return
    }
    if (!address.startsWith('bth1') || address.length < 40) {
      setError('Invalid address format. Should start with bth1...')
      return
    }
    onComplete(address)
  }

  return (
    <Card>
      <CardContent className="flex flex-col items-center justify-center py-12">
        <motion.div
          initial={{ scale: 0.8, opacity: 0 }}
          animate={{ scale: 1, opacity: 1 }}
          className="mb-6 flex h-20 w-20 items-center justify-center rounded-2xl bg-gradient-to-br from-[--color-pulse]/20 to-[--color-purple]/20"
        >
          <Wallet className="h-10 w-10 text-[--color-pulse]" />
        </motion.div>

        <h2 className="font-display text-xl font-bold text-[--color-light]">
          Connect Your Wallet
        </h2>
        <p className="mt-2 max-w-sm text-center text-sm text-[--color-ghost]">
          Enter your Botho wallet address to view your balance and transaction history.
        </p>

        <div className="mt-6 w-full max-w-md space-y-4">
          <Input
            placeholder="bth1qxy2kgdyg..."
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            error={error || undefined}
            className="font-mono text-center"
          />
          <Button onClick={handleSubmit} className="w-full">
            <Lock className="h-4 w-4" />
            Connect Address
          </Button>
        </div>

        <p className="mt-4 text-xs text-[--color-dim]">
          Your address is stored locally and never shared.
        </p>
      </CardContent>
    </Card>
  )
}

// Main wallet page
export function WalletPage() {
  const { connectedNode } = useConnection()
  const {
    address,
    balance,
    transactions,
    isLoading,
    error,
    refreshBalance,
    refreshTransactions,
    setAddress,
  } = useWallet()

  const [showSendModal, setShowSendModal] = useState(false)
  const [copied, setCopied] = useState(false)

  const copyAddress = useCallback(() => {
    if (address) {
      navigator.clipboard.writeText(address)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    }
  }, [address])

  // If not connected to node
  if (!connectedNode) {
    return (
      <Layout title="Wallet" subtitle="Manage your BTH holdings">
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-16">
            <div className="mb-4 flex h-16 w-16 items-center justify-center rounded-2xl bg-[--color-warning]/10">
              <Zap className="h-8 w-8 text-[--color-warning]" />
            </div>
            <h2 className="font-display text-lg font-bold text-[--color-light]">
              No Node Connected
            </h2>
            <p className="mt-2 text-sm text-[--color-ghost]">
              Connect to a Botho node to access your wallet.
            </p>
          </CardContent>
        </Card>
      </Layout>
    )
  }

  // If no address set
  if (!address) {
    return (
      <Layout title="Wallet" subtitle="Manage your BTH holdings">
        <AddressSetup onComplete={setAddress} />
      </Layout>
    )
  }

  return (
    <Layout title="Wallet" subtitle="Manage your BTH holdings">
      <div className="space-y-6">
        {/* Balance Card */}
        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
        >
          <Card className="overflow-hidden">
            {/* Gradient background */}
            <div className="absolute inset-0 bg-gradient-to-br from-[--color-pulse]/5 via-transparent to-[--color-purple]/5" />

            <CardContent className="relative">
              <div className="flex flex-col gap-6 md:flex-row md:items-center md:justify-between">
                {/* Balance */}
                <div>
                  <div className="flex items-center gap-2 text-sm text-[--color-ghost]">
                    <Wallet className="h-4 w-4" />
                    Total Balance
                  </div>
                  <div className="mt-1 font-display text-4xl font-bold tracking-tight text-[--color-light]">
                    {balance ? (
                      <motion.span
                        key={balance.total.toString()}
                        initial={{ opacity: 0, y: 10 }}
                        animate={{ opacity: 1, y: 0 }}
                      >
                        {formatBTH(balance.total)}
                        <span className="ml-2 text-xl text-[--color-ghost]">BTH</span>
                      </motion.span>
                    ) : (
                      <span className="animate-pulse text-[--color-dim]">Loading...</span>
                    )}
                  </div>

                  {/* Sub-balances */}
                  {balance && (
                    <div className="mt-3 flex gap-6 text-sm">
                      <div>
                        <span className="text-[--color-dim]">Available: </span>
                        <span className="font-mono text-[--color-success]">
                          {formatBTH(balance.available)}
                        </span>
                      </div>
                      {balance.pending > 0 && (
                        <div>
                          <span className="text-[--color-dim]">Pending: </span>
                          <span className="font-mono text-[--color-warning]">
                            {formatBTH(balance.pending)}
                          </span>
                        </div>
                      )}
                    </div>
                  )}
                </div>

                {/* Actions */}
                <div className="flex gap-3">
                  <Button
                    variant="secondary"
                    onClick={() => {
                      refreshBalance()
                      refreshTransactions()
                    }}
                    disabled={isLoading}
                  >
                    <RefreshCw className={`h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
                    Refresh
                  </Button>
                  <Button onClick={() => setShowSendModal(true)}>
                    <Send className="h-4 w-4" />
                    Send
                  </Button>
                </div>
              </div>

              {/* Address */}
              <div className="mt-6 flex items-center gap-2 rounded-lg border border-[--color-steel] bg-[--color-slate]/50 p-3">
                <span className="text-sm text-[--color-dim]">Address:</span>
                <code className="flex-1 truncate font-mono text-sm text-[--color-ghost]">
                  {address}
                </code>
                <button
                  onClick={copyAddress}
                  className="rounded-md p-1.5 text-[--color-dim] transition-colors hover:bg-[--color-steel] hover:text-[--color-light]"
                >
                  {copied ? (
                    <Check className="h-4 w-4 text-[--color-success]" />
                  ) : (
                    <Copy className="h-4 w-4" />
                  )}
                </button>
              </div>
            </CardContent>
          </Card>
        </motion.div>

        {/* Error */}
        {error && (
          <motion.div
            initial={{ opacity: 0, y: -10 }}
            animate={{ opacity: 1, y: 0 }}
            className="rounded-lg border border-[--color-danger]/30 bg-[--color-danger]/10 p-4 text-sm text-[--color-danger]"
          >
            {error}
          </motion.div>
        )}

        {/* Transactions */}
        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.1 }}
        >
          <Card>
            <CardHeader>
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <Clock className="h-4 w-4 text-[--color-pulse]" />
                  <CardTitle>Transaction History</CardTitle>
                </div>
                <span className="text-sm text-[--color-dim]">
                  {transactions.length} transactions
                </span>
              </div>
            </CardHeader>
            <CardContent>
              {transactions.length === 0 ? (
                <div className="py-12 text-center">
                  <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-xl bg-[--color-slate]">
                    <Sparkles className="h-6 w-6 text-[--color-dim]" />
                  </div>
                  <p className="text-[--color-ghost]">No transactions yet</p>
                  <p className="mt-1 text-sm text-[--color-dim]">
                    Transactions will appear here once you send or receive BTH.
                  </p>
                </div>
              ) : (
                <div className="space-y-2">
                  {transactions.map((tx, i) => (
                    <TransactionRow key={tx.id} tx={tx} index={i} />
                  ))}
                </div>
              )}
            </CardContent>
          </Card>
        </motion.div>
      </div>

      {/* Send Modal */}
      <SendModal isOpen={showSendModal} onClose={() => setShowSendModal(false)} />
    </Layout>
  )
}
