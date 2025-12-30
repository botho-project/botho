import { useState, useCallback } from 'react'
import { Layout } from '../components/layout'
import { Card, CardContent, Button, Input } from '@botho/ui'
import {
  BalanceCard,
  TransactionList,
  SendModal,
  type SendFormData,
  type SendResult,
} from '@botho/features'
import { motion, AnimatePresence } from 'motion/react'
import { FileKey, Key, Lock, RefreshCw, Save, Send, Timer, Unlock, Wallet, Zap } from 'lucide-react'
import { useWallet } from '../contexts/wallet'
import { useConnection } from '../contexts/connection'
import { isValidMnemonic } from '@botho/core'

// SECURITY NOTE: This component NEVER handles raw mnemonics for transaction signing.
// Mnemonics are only used during initial wallet creation (create_wallet command),
// then immediately encrypted and stored. All subsequent operations use the
// session-based API where keys stay in Rust memory.

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

type UnlockMode = 'choose' | 'file' | 'create'

// Wallet unlock component with file support
// SECURITY: Mnemonic only handled during wallet creation, then kept in Rust
function WalletUnlock({
  onUnlocked,
  onCancel,
  hasWalletFile,
  walletFilePath,
  onUnlockWithPassword,
  onCreateWallet,
}: {
  onUnlocked: () => void
  onCancel: () => void
  hasWalletFile: boolean
  walletFilePath: string | null
  onUnlockWithPassword: (password: string) => Promise<{ success: boolean; error?: string }>
  onCreateWallet: (mnemonic: string, password: string) => Promise<{ success: boolean; error?: string }>
}) {
  const [mode, setMode] = useState<UnlockMode>(hasWalletFile ? 'file' : 'create')
  const [mnemonic, setMnemonic] = useState('')
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [isLoading, setIsLoading] = useState(false)

  const handleUnlockFromFile = async () => {
    if (!password) {
      setError('Please enter your password')
      return
    }

    setIsLoading(true)
    setError(null)

    const result = await onUnlockWithPassword(password)

    setIsLoading(false)

    if (result.success) {
      onUnlocked()
    } else {
      setError(result.error || 'Failed to unlock wallet')
    }
  }

  const handleCreateWallet = async () => {
    const trimmed = mnemonic.trim()

    if (!trimmed) {
      setError('Please enter your recovery phrase')
      return
    }

    if (!isValidMnemonic(trimmed)) {
      setError('Invalid recovery phrase. Please check your words.')
      return
    }

    if (!password) {
      setError('Please enter a password')
      return
    }

    if (password.length < 8) {
      setError('Password must be at least 8 characters')
      return
    }

    if (password !== confirmPassword) {
      setError('Passwords do not match')
      return
    }

    setIsLoading(true)
    setError(null)

    // SECURITY: Mnemonic is sent to Rust ONCE for wallet creation,
    // then encrypted and stored. Keys stay in Rust memory.
    const result = await onCreateWallet(trimmed, password)

    setIsLoading(false)

    if (result.success) {
      // Clear mnemonic from JS memory after successful creation
      setMnemonic('')
      onUnlocked()
    } else {
      setError(result.error || 'Failed to create wallet')
    }
  }

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={onCancel}
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.95, y: 20 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        exit={{ opacity: 0, scale: 0.95, y: 20 }}
        onClick={(e) => e.stopPropagation()}
        className="relative w-full max-w-md rounded-2xl border border-[--color-steel] bg-[--color-abyss] p-6 shadow-2xl"
      >
        <div className="mb-6 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-[--color-warning]/20">
            <Lock className="h-5 w-5 text-[--color-warning]" />
          </div>
          <div>
            <h2 className="font-display text-lg font-bold text-[--color-light]">
              {hasWalletFile ? 'Unlock Wallet' : 'Create Wallet'}
            </h2>
            <p className="text-sm text-[--color-dim]">
              {mode === 'choose' && 'Choose how to proceed'}
              {mode === 'file' && 'Enter your wallet password'}
              {mode === 'create' && 'Create a new encrypted wallet'}
            </p>
          </div>
        </div>

        <AnimatePresence mode="wait">
          {/* Choose mode - only shown if wallet file exists */}
          {mode === 'choose' && hasWalletFile && (
            <motion.div
              key="choose"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 20 }}
              className="space-y-3"
            >
              <button
                onClick={() => setMode('file')}
                className="flex w-full items-center gap-4 rounded-xl border border-[--color-steel] bg-[--color-void] p-4 text-left transition-colors hover:border-[--color-pulse] hover:bg-[--color-steel]/30"
              >
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-pulse]/20">
                  <FileKey className="h-5 w-5 text-[--color-pulse]" />
                </div>
                <div>
                  <div className="font-medium text-[--color-light]">Unlock Wallet</div>
                  <div className="text-xs text-[--color-dim]">
                    Use your encrypted wallet file
                  </div>
                </div>
              </button>

              <button
                onClick={() => setMode('create')}
                className="flex w-full items-center gap-4 rounded-xl border border-[--color-steel] bg-[--color-void] p-4 text-left transition-colors hover:border-[--color-pulse] hover:bg-[--color-steel]/30"
              >
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-ghost]/20">
                  <Key className="h-5 w-5 text-[--color-ghost]" />
                </div>
                <div>
                  <div className="font-medium text-[--color-light]">Import Different Wallet</div>
                  <div className="text-xs text-[--color-dim]">
                    Create new wallet from recovery phrase
                  </div>
                </div>
              </button>

              <div className="pt-2">
                <Button variant="secondary" onClick={onCancel} className="w-full">
                  Cancel
                </Button>
              </div>
            </motion.div>
          )}

          {/* File unlock mode */}
          {mode === 'file' && (
            <motion.div
              key="file"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 20 }}
              className="space-y-4"
            >
              <div>
                <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                  Wallet Password
                </label>
                <Input
                  type="password"
                  placeholder="Enter your password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && handleUnlockFromFile()}
                />
                {error && <p className="mt-1 text-sm text-[--color-danger]">{error}</p>}
              </div>

              {walletFilePath && (
                <p className="text-xs text-[--color-dim]">
                  Loading from: <span className="font-mono">{walletFilePath}</span>
                </p>
              )}

              <div className="flex gap-3">
                <Button
                  variant="secondary"
                  onClick={() => {
                    setMode('choose')
                    setError(null)
                    setPassword('')
                  }}
                  className="flex-1"
                >
                  Back
                </Button>
                <Button onClick={handleUnlockFromFile} disabled={isLoading} className="flex-1">
                  {isLoading ? (
                    <RefreshCw className="h-4 w-4 animate-spin" />
                  ) : (
                    <Unlock className="h-4 w-4" />
                  )}
                  Unlock
                </Button>
              </div>
            </motion.div>
          )}

          {/* Create wallet mode - enter mnemonic and encrypt with password */}
          {mode === 'create' && (
            <motion.div
              key="create"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 20 }}
              className="space-y-4"
            >
              <div>
                <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                  Recovery Phrase (24 words)
                </label>
                <textarea
                  placeholder="word1 word2 word3 ... word24"
                  value={mnemonic}
                  onChange={(e) => setMnemonic(e.target.value)}
                  className="h-20 w-full resize-none rounded-xl border border-[--color-steel] bg-[--color-void] px-4 py-3 font-mono text-sm text-[--color-light] placeholder:text-[--color-dim] focus:border-[--color-pulse] focus:outline-none"
                />
              </div>

              <div>
                <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                  Create Password
                </label>
                <Input
                  type="password"
                  placeholder="At least 8 characters"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
              </div>

              <div>
                <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                  Confirm Password
                </label>
                <Input
                  type="password"
                  placeholder="Confirm your password"
                  value={confirmPassword}
                  onChange={(e) => setConfirmPassword(e.target.value)}
                />
                {error && <p className="mt-1 text-sm text-[--color-danger]">{error}</p>}
              </div>

              <p className="text-xs text-[--color-dim]">
                Your wallet will be encrypted with Argon2 + ChaCha20-Poly1305 and saved locally.
                The recovery phrase is only used once to create the wallet, then stays securely in Rust memory.
              </p>

              <div className="flex gap-3">
                {hasWalletFile ? (
                  <Button
                    variant="secondary"
                    onClick={() => {
                      setMode('choose')
                      setError(null)
                      setMnemonic('')
                      setPassword('')
                      setConfirmPassword('')
                    }}
                    className="flex-1"
                  >
                    Back
                  </Button>
                ) : (
                  <Button variant="secondary" onClick={onCancel} className="flex-1">
                    Cancel
                  </Button>
                )}
                <Button onClick={handleCreateWallet} disabled={isLoading} className="flex-1">
                  {isLoading ? (
                    <RefreshCw className="h-4 w-4 animate-spin" />
                  ) : (
                    <Save className="h-4 w-4" />
                  )}
                  Create & Unlock
                </Button>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </motion.div>
    </motion.div>
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
    isSending,
    isUnlocked,
    hasWalletFile,
    walletFilePath,
    sessionExpiresIn,
    error,
    refreshBalance,
    refreshTransactions,
    sendTransaction,
    estimateFee,
    setAddress,
    unlockWallet,
    createWallet,
    lockWallet,
  } = useWallet()

  const [showSendModal, setShowSendModal] = useState(false)
  const [showUnlockModal, setShowUnlockModal] = useState(false)

  // Handle send button click
  const handleSendClick = () => {
    if (!isUnlocked) {
      setShowUnlockModal(true)
    } else {
      setShowSendModal(true)
    }
  }

  // Handle wallet unlocked (session-based - mnemonic stays in Rust)
  const handleUnlocked = () => {
    setShowUnlockModal(false)
    setShowSendModal(true)
  }

  // Handle unlock with password (keys stay in Rust)
  const handleUnlockWithPassword = async (password: string) => {
    return unlockWallet(password)
  }

  // Handle create wallet (mnemonic sent to Rust once, then encrypted)
  const handleCreateWallet = async (mnemonic: string, password: string) => {
    return createWallet(mnemonic, password)
  }

  // Wrap sendTransaction to match SendModal interface
  const handleSend = useCallback(
    async (data: SendFormData): Promise<SendResult> => {
      return sendTransaction({
        recipient: data.recipient,
        amount: data.amount,
        privacyLevel: data.privacyLevel,
        memo: data.memo,
        customFee: data.customFee,
      })
    },
    [sendTransaction]
  )

  // Wrap estimateFee to match SendModal interface
  const handleEstimateFee = useCallback(
    async (amount: bigint, privacyLevel: 'standard' | 'private'): Promise<bigint> => {
      return estimateFee(amount, privacyLevel)
    },
    [estimateFee]
  )

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
        <BalanceCard
          balance={balance}
          address={address}
          isLoading={isLoading}
          actions={
            <>
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
              <Button onClick={handleSendClick}>
                <Send className="h-4 w-4" />
                Send
              </Button>
              {isUnlocked && (
                <Button variant="ghost" onClick={lockWallet} title="Lock wallet">
                  <Lock className="h-4 w-4" />
                </Button>
              )}
            </>
          }
        />

        {/* Unlock status indicator with session expiry */}
        {isUnlocked && (
          <motion.div
            initial={{ opacity: 0, y: -10 }}
            animate={{ opacity: 1, y: 0 }}
            className="flex items-center justify-between rounded-lg border border-[--color-pulse]/30 bg-[--color-pulse]/10 px-4 py-2 text-sm text-[--color-pulse]"
          >
            <div className="flex items-center gap-2">
              <Unlock className="h-4 w-4" />
              Wallet unlocked for sending
            </div>
            {sessionExpiresIn !== null && (
              <div className="flex items-center gap-1 text-xs text-[--color-ghost]">
                <Timer className="h-3 w-3" />
                {Math.floor(sessionExpiresIn / 60)}m remaining
              </div>
            )}
          </motion.div>
        )}

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
          <TransactionList transactions={transactions} />
        </motion.div>
      </div>

      {/* Unlock Modal */}
      {showUnlockModal && (
        <WalletUnlock
          onUnlocked={handleUnlocked}
          onCancel={() => setShowUnlockModal(false)}
          hasWalletFile={hasWalletFile}
          walletFilePath={walletFilePath}
          onUnlockWithPassword={handleUnlockWithPassword}
          onCreateWallet={handleCreateWallet}
        />
      )}

      {/* Send Modal */}
      <SendModal
        isOpen={showSendModal}
        onClose={() => setShowSendModal(false)}
        balance={balance}
        estimateFee={handleEstimateFee}
        onSend={handleSend}
        isSending={isSending}
      />
    </Layout>
  )
}
