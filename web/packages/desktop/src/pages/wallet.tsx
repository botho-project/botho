import { useState, useCallback, useEffect, useMemo } from 'react'
import { Layout, FaucetButton } from '../components'
import { Card, CardContent, Button, Input } from '@botho/ui'
import {
  BalanceCard,
  TransactionList,
  SendModal,
  type SendFormData,
  type SendResult,
} from '@botho/features'
import { motion, AnimatePresence } from 'motion/react'
import { Check, FileKey, Key, Lock, Plus, RefreshCw, Save, Send, Timer, Unlock, Wallet, Zap } from 'lucide-react'
import { useWallet } from '../contexts/wallet'
import { useConnection } from '../contexts/connection'
import { isValidMnemonic } from '@botho/core'
import { NETWORKS, hasFaucetSupport, type NetworkConfig } from '../config/networks'

// SECURITY NOTE: For NEW wallets, the mnemonic is generated in Rust and only displayed
// to the user for backup. The mnemonic is NEVER sent from JS back to Rust.
// For IMPORTED wallets, the mnemonic must cross the JS/Rust boundary (unavoidable for restore).
// All subsequent operations use the session-based API where keys stay in Rust memory.

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

// Mode for wallet setup flow
type UnlockMode = 'choose' | 'file' | 'new-display' | 'new-verify' | 'new-password' | 'import'

// Wallet unlock component with secure creation flow
// SECURITY: For NEW wallets, mnemonic is generated in Rust and NEVER sent from JS.
// For IMPORTED wallets, mnemonic must cross JS/Rust boundary (unavoidable for restore).
function WalletUnlock({
  onUnlocked,
  onCancel,
  hasWalletFile,
  walletFilePath,
  onUnlockWithPassword,
  onGenerateMnemonic,
  onConfirmNewWallet,
  onCancelPendingWallet,
  onImportWallet,
}: {
  onUnlocked: () => void
  onCancel: () => void
  hasWalletFile: boolean
  walletFilePath: string | null
  onUnlockWithPassword: (password: string) => Promise<{ success: boolean; error?: string }>
  onGenerateMnemonic: () => Promise<{ success: boolean; words?: string[]; verifyPositions?: number[]; error?: string }>
  onConfirmNewWallet: (password: string, verifyWords: string[]) => Promise<{ success: boolean; error?: string }>
  onCancelPendingWallet: () => Promise<void>
  onImportWallet: (mnemonic: string, password: string) => Promise<{ success: boolean; error?: string }>
}) {
  const [mode, setMode] = useState<UnlockMode>(hasWalletFile ? 'file' : 'choose')
  // For new wallet flow
  const [generatedWords, setGeneratedWords] = useState<string[]>([])
  const [verifyPositions, setVerifyPositions] = useState<number[]>([])
  const [verifyWords, setVerifyWords] = useState<string[]>(['', '', ''])
  const [hasWrittenDown, setHasWrittenDown] = useState(false)
  // For import flow
  const [importMnemonic, setImportMnemonic] = useState('')
  // Common
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [isLoading, setIsLoading] = useState(false)

  // Clean up pending wallet on unmount or cancel
  useEffect(() => {
    return () => {
      if (generatedWords.length > 0) {
        onCancelPendingWallet()
      }
    }
  }, [generatedWords.length, onCancelPendingWallet])

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

  // Start new wallet creation - generate mnemonic in Rust
  const handleStartNewWallet = async () => {
    setIsLoading(true)
    setError(null)

    const result = await onGenerateMnemonic()

    setIsLoading(false)

    if (result.success && result.words && result.verifyPositions) {
      setGeneratedWords(result.words)
      setVerifyPositions(result.verifyPositions)
      setMode('new-display')
    } else {
      setError(result.error || 'Failed to generate wallet')
    }
  }

  // Move to verification step
  const handleProceedToVerify = () => {
    if (!hasWrittenDown) {
      setError('Please confirm you have written down your recovery phrase')
      return
    }
    setError(null)
    setMode('new-verify')
  }

  // Verify words and move to password step
  const handleVerifyWords = () => {
    // Check all verify words are filled
    if (verifyWords.some(w => !w.trim())) {
      setError('Please enter all verification words')
      return
    }
    setError(null)
    setMode('new-password')
  }

  // Complete new wallet creation
  const handleCompleteNewWallet = async () => {
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

    // SECURITY: Mnemonic is NEVER sent from JS - Rust uses cached mnemonic
    const result = await onConfirmNewWallet(password, verifyWords.map(w => w.trim()))

    setIsLoading(false)

    if (result.success) {
      // Clear local state
      setGeneratedWords([])
      setVerifyPositions([])
      setVerifyWords(['', '', ''])
      onUnlocked()
    } else {
      // If verification failed, go back to verify step
      if (result.error?.includes('incorrect')) {
        setMode('new-verify')
      }
      setError(result.error || 'Failed to create wallet')
    }
  }

  // Handle import wallet
  const handleImportWallet = async () => {
    const trimmed = importMnemonic.trim()

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

    const result = await onImportWallet(trimmed, password)

    setIsLoading(false)

    if (result.success) {
      setImportMnemonic('')
      onUnlocked()
    } else {
      setError(result.error || 'Failed to import wallet')
    }
  }

  const handleCancel = () => {
    if (generatedWords.length > 0) {
      onCancelPendingWallet()
    }
    onCancel()
  }

  const resetToChoose = () => {
    if (generatedWords.length > 0) {
      onCancelPendingWallet()
    }
    setGeneratedWords([])
    setVerifyPositions([])
    setVerifyWords(['', '', ''])
    setHasWrittenDown(false)
    setImportMnemonic('')
    setPassword('')
    setConfirmPassword('')
    setError(null)
    setMode(hasWalletFile ? 'choose' : 'choose')
  }

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={handleCancel}
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
              {mode === 'file' && 'Unlock Wallet'}
              {mode === 'choose' && (hasWalletFile ? 'Wallet Options' : 'Create Wallet')}
              {mode === 'new-display' && 'Write Down Your Recovery Phrase'}
              {mode === 'new-verify' && 'Verify Recovery Phrase'}
              {mode === 'new-password' && 'Set Wallet Password'}
              {mode === 'import' && 'Import Existing Wallet'}
            </h2>
            <p className="text-sm text-[--color-dim]">
              {mode === 'file' && 'Enter your wallet password'}
              {mode === 'choose' && 'Choose how to proceed'}
              {mode === 'new-display' && 'Write these 24 words in order'}
              {mode === 'new-verify' && 'Enter the words at the specified positions'}
              {mode === 'new-password' && 'Encrypt your wallet with a password'}
              {mode === 'import' && 'Restore from your 24-word recovery phrase'}
            </p>
          </div>
        </div>

        <AnimatePresence mode="wait">
          {/* Choose mode */}
          {mode === 'choose' && (
            <motion.div
              key="choose"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 20 }}
              className="space-y-3"
            >
              {hasWalletFile && (
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
              )}

              <button
                onClick={handleStartNewWallet}
                disabled={isLoading}
                className="flex w-full items-center gap-4 rounded-xl border border-[--color-steel] bg-[--color-void] p-4 text-left transition-colors hover:border-[--color-pulse] hover:bg-[--color-steel]/30 disabled:opacity-50"
              >
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-success]/20">
                  <Plus className="h-5 w-5 text-[--color-success]" />
                </div>
                <div>
                  <div className="font-medium text-[--color-light]">Create New Wallet</div>
                  <div className="text-xs text-[--color-dim]">
                    Generate a new secure recovery phrase
                  </div>
                </div>
              </button>

              <button
                onClick={() => setMode('import')}
                className="flex w-full items-center gap-4 rounded-xl border border-[--color-steel] bg-[--color-void] p-4 text-left transition-colors hover:border-[--color-pulse] hover:bg-[--color-steel]/30"
              >
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-ghost]/20">
                  <Key className="h-5 w-5 text-[--color-ghost]" />
                </div>
                <div>
                  <div className="font-medium text-[--color-light]">Import Existing Wallet</div>
                  <div className="text-xs text-[--color-dim]">
                    Restore from recovery phrase
                  </div>
                </div>
              </button>

              {error && <p className="text-sm text-[--color-danger]">{error}</p>}

              <div className="pt-2">
                <Button variant="secondary" onClick={handleCancel} className="w-full">
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
                  onClick={resetToChoose}
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

          {/* New wallet - display mnemonic */}
          {mode === 'new-display' && (
            <motion.div
              key="new-display"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 20 }}
              className="space-y-4"
            >
              <div className="rounded-xl border border-[--color-warning]/30 bg-[--color-warning]/10 p-3">
                <p className="text-xs text-[--color-warning]">
                  Write these words down in order and store them securely.
                  This is the ONLY way to recover your wallet. Never share them with anyone.
                </p>
              </div>

              <div className="grid grid-cols-3 gap-2 rounded-xl border border-[--color-steel] bg-[--color-void] p-4">
                {generatedWords.map((word, i) => (
                  <div key={i} className="flex items-center gap-2 text-sm">
                    <span className="w-5 text-right text-[--color-dim]">{i + 1}.</span>
                    <span className="font-mono text-[--color-light]">{word}</span>
                  </div>
                ))}
              </div>

              <label className="flex items-center gap-3 rounded-lg border border-[--color-steel] p-3 cursor-pointer hover:bg-[--color-steel]/20">
                <input
                  type="checkbox"
                  checked={hasWrittenDown}
                  onChange={(e) => setHasWrittenDown(e.target.checked)}
                  className="h-4 w-4 rounded border-[--color-steel] bg-[--color-void] text-[--color-pulse] focus:ring-[--color-pulse]"
                />
                <span className="text-sm text-[--color-ghost]">
                  I have written down my recovery phrase in a safe place
                </span>
              </label>

              {error && <p className="text-sm text-[--color-danger]">{error}</p>}

              <div className="flex gap-3">
                <Button variant="secondary" onClick={resetToChoose} className="flex-1">
                  Cancel
                </Button>
                <Button onClick={handleProceedToVerify} className="flex-1">
                  Continue
                </Button>
              </div>
            </motion.div>
          )}

          {/* New wallet - verify words */}
          {mode === 'new-verify' && (
            <motion.div
              key="new-verify"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 20 }}
              className="space-y-4"
            >
              <p className="text-sm text-[--color-ghost]">
                Enter the words at the following positions to verify you saved your recovery phrase:
              </p>

              <div className="space-y-3">
                {verifyPositions.map((pos, i) => (
                  <div key={i}>
                    <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
                      Word #{pos}
                    </label>
                    <Input
                      type="text"
                      placeholder={`Enter word #${pos}`}
                      value={verifyWords[i]}
                      onChange={(e) => {
                        const newWords = [...verifyWords]
                        newWords[i] = e.target.value
                        setVerifyWords(newWords)
                      }}
                      className="font-mono"
                    />
                  </div>
                ))}
              </div>

              {error && <p className="text-sm text-[--color-danger]">{error}</p>}

              <div className="flex gap-3">
                <Button
                  variant="secondary"
                  onClick={() => {
                    setError(null)
                    setMode('new-display')
                  }}
                  className="flex-1"
                >
                  Back
                </Button>
                <Button onClick={handleVerifyWords} className="flex-1">
                  <Check className="h-4 w-4" />
                  Verify
                </Button>
              </div>
            </motion.div>
          )}

          {/* New wallet - set password */}
          {mode === 'new-password' && (
            <motion.div
              key="new-password"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 20 }}
              className="space-y-4"
            >
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
              </div>

              {error && <p className="text-sm text-[--color-danger]">{error}</p>}

              <p className="text-xs text-[--color-dim]">
                Your wallet will be encrypted with Argon2 + ChaCha20-Poly1305 and saved locally.
              </p>

              <div className="flex gap-3">
                <Button
                  variant="secondary"
                  onClick={() => {
                    setError(null)
                    setMode('new-verify')
                  }}
                  className="flex-1"
                >
                  Back
                </Button>
                <Button onClick={handleCompleteNewWallet} disabled={isLoading} className="flex-1">
                  {isLoading ? (
                    <RefreshCw className="h-4 w-4 animate-spin" />
                  ) : (
                    <Save className="h-4 w-4" />
                  )}
                  Create Wallet
                </Button>
              </div>
            </motion.div>
          )}

          {/* Import wallet mode */}
          {mode === 'import' && (
            <motion.div
              key="import"
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
                  value={importMnemonic}
                  onChange={(e) => setImportMnemonic(e.target.value)}
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
              </div>

              {error && <p className="text-sm text-[--color-danger]">{error}</p>}

              <p className="text-xs text-[--color-dim]">
                Your wallet will be encrypted with Argon2 + ChaCha20-Poly1305 and saved locally.
              </p>

              <div className="flex gap-3">
                <Button variant="secondary" onClick={resetToChoose} className="flex-1">
                  Back
                </Button>
                <Button onClick={handleImportWallet} disabled={isLoading} className="flex-1">
                  {isLoading ? (
                    <RefreshCw className="h-4 w-4 animate-spin" />
                  ) : (
                    <Save className="h-4 w-4" />
                  )}
                  Import & Unlock
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

  // Determine current network configuration based on connected node
  const networkConfig = useMemo<NetworkConfig | null>(() => {
    if (!connectedNode) return null

    // Check if connected to a known network
    for (const network of Object.values(NETWORKS)) {
      if (
        connectedNode.host === network.rpcHost ||
        connectedNode.host.includes(network.rpcHost)
      ) {
        return network
      }
    }

    // Default to testnet config if connected to seed.botho.io
    if (connectedNode.host.includes('botho.io')) {
      return NETWORKS.testnet
    }

    // Default to local if localhost
    if (connectedNode.host === '127.0.0.1' || connectedNode.host === 'localhost') {
      return NETWORKS.local
    }

    return null
  }, [connectedNode])

  // Check if faucet is available for current network
  const faucetAvailable = networkConfig && hasFaucetSupport(networkConfig)

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
    generateMnemonic,
    confirmNewWallet,
    cancelPendingWallet,
    importWallet,
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

  // Handle generate mnemonic (SECURE - generated in Rust)
  const handleGenerateMnemonic = async () => {
    return generateMnemonic()
  }

  // Handle confirm new wallet (SECURE - mnemonic NEVER sent from JS)
  const handleConfirmNewWallet = async (password: string, verifyWords: string[]) => {
    return confirmNewWallet(password, verifyWords)
  }

  // Handle cancel pending wallet
  const handleCancelPendingWallet = async () => {
    return cancelPendingWallet()
  }

  // Handle import wallet (mnemonic crosses JS/Rust - unavoidable for restore)
  const handleImportWallet = async (mnemonic: string, password: string) => {
    return importWallet(mnemonic, password)
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
              {faucetAvailable && networkConfig && (
                <FaucetButton
                  faucetHost={networkConfig.faucetHost!}
                  faucetPort={networkConfig.faucetPort!}
                  isUnlocked={isUnlocked}
                  onUnlockRequired={() => setShowUnlockModal(true)}
                  onSuccess={() => {
                    refreshBalance()
                    refreshTransactions()
                  }}
                />
              )}
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
          onGenerateMnemonic={handleGenerateMnemonic}
          onConfirmNewWallet={handleConfirmNewWallet}
          onCancelPendingWallet={handleCancelPendingWallet}
          onImportWallet={handleImportWallet}
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
