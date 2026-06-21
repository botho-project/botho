import { useState, useMemo } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, Input, Logo } from '@botho/ui'
import { createMnemonic12, MIN_PASSWORD_LENGTH, passwordStrength, type PasswordStrength } from '@botho/core'
import { BalanceCard, TransactionList, SendModal, type SendFormData, type SendResult } from '@botho/features'
import { useWallet } from '../contexts/wallet'
import { useNetwork } from '../contexts/network'
import { NetworkSelector } from '../components/NetworkSelector'
import { FaucetButton } from '../components/FaucetButton'
import { SendLinkModal } from '../components/SendLinkModal'
import { RequestModal } from '../components/RequestModal'
import { ReceiveModal } from '../components/ReceiveModal'
import { OutstandingLinks } from '../components/OutstandingLinks'
import { Send, Link2, Download, RefreshCw, ArrowLeft, Shield, Eye, KeyRound, AlertCircle, Lock, Settings, Trash2, Users, QrCode } from 'lucide-react'

const STRENGTH_META: Record<PasswordStrength, { label: string; bars: number; color: string }> = {
  'too-short': { label: `At least ${MIN_PASSWORD_LENGTH} characters`, bars: 0, color: 'bg-danger' },
  weak: { label: 'Weak password', bars: 1, color: 'bg-danger' },
  fair: { label: 'Fair password', bars: 2, color: 'bg-warning' },
  strong: { label: 'Strong password', bars: 3, color: 'bg-success' },
}

/**
 * Shared password + confirm fields with a simple strength hint (#475). Encrypts
 * the wallet by default; a password is required to proceed.
 */
function PasswordFields({
  password,
  confirmPassword,
  onPassword,
  onConfirmPassword,
}: {
  password: string
  confirmPassword: string
  onPassword: (v: string) => void
  onConfirmPassword: (v: string) => void
}) {
  const passwordsMatch = password === confirmPassword
  const strength = passwordStrength(password)
  const meta = STRENGTH_META[strength]

  return (
    <div className="space-y-3">
      <div>
        <Input
          type="password"
          placeholder={`Password (min ${MIN_PASSWORD_LENGTH} characters)`}
          value={password}
          onChange={(e: React.ChangeEvent<HTMLInputElement>) => onPassword(e.target.value)}
        />
        {password.length > 0 && (
          <div className="mt-2">
            <div className="flex gap-1">
              {[0, 1, 2].map((i) => (
                <div
                  key={i}
                  className={`h-1 flex-1 rounded-full ${i < meta.bars ? meta.color : 'bg-steel'}`}
                />
              ))}
            </div>
            <p className="text-xs text-ghost mt-1">{meta.label}</p>
          </div>
        )}
      </div>
      <div>
        <Input
          type="password"
          placeholder="Confirm password"
          value={confirmPassword}
          onChange={(e: React.ChangeEvent<HTMLInputElement>) => onConfirmPassword(e.target.value)}
        />
        {confirmPassword && !passwordsMatch && (
          <p className="text-xs text-danger mt-1">Passwords don't match</p>
        )}
      </div>
    </div>
  )
}

/** True when a password is valid to encrypt a wallet (#475). */
function isPasswordValid(password: string, confirmPassword: string): boolean {
  return password.length >= MIN_PASSWORD_LENGTH && password === confirmPassword
}

function CreateWalletView({ onCreate }: { onCreate: (mnemonic: string, password?: string) => void }) {
  const [showMnemonic, setShowMnemonic] = useState(false)
  const [confirmed, setConfirmed] = useState(false)
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  // Generate mnemonic once and keep it stable
  const mnemonic = useMemo(() => createMnemonic12(), [])

  // SECURITY (#475): a password is REQUIRED — the seed is always encrypted at
  // rest. There is no plaintext opt-out in the UI.
  const passwordValid = isPasswordValid(password, confirmPassword)

  const handleCreate = () => {
    onCreate(mnemonic, password)
  }

  return (
    <div className="max-w-lg mx-auto px-4 sm:px-0">
      <Card className="p-5 sm:p-8">
        <div className="text-center mb-6 sm:mb-8">
          <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
            <Shield className="text-pulse" size={28} />
          </div>
          <h2 className="font-display text-xl sm:text-2xl font-bold mb-2">Create New Wallet</h2>
          <p className="text-ghost text-sm sm:text-base">Write down your recovery phrase and store it safely.</p>
        </div>

        <div className="relative mb-5 sm:mb-6">
          <div className={`p-3 sm:p-4 rounded-lg bg-abyss border border-steel font-mono text-xs sm:text-sm leading-relaxed ${showMnemonic ? '' : 'blur-sm select-none'}`}>
            {mnemonic}
          </div>
          {!showMnemonic && (
            <button
              onClick={() => setShowMnemonic(true)}
              className="absolute inset-0 flex items-center justify-center gap-2 text-ghost hover:text-light transition-colors"
            >
              <Eye size={20} />
              <span className="text-sm">Click to reveal</span>
            </button>
          )}
        </div>

        <label className="flex items-start gap-3 mb-4 cursor-pointer">
          <input type="checkbox" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} className="mt-1 w-4 h-4 accent-pulse" />
          <span className="text-sm text-ghost">
            I have written down my recovery phrase and stored it in a safe place.
            <span className="text-danger ml-1">*</span>
          </span>
        </label>

        <div className="border-t border-steel pt-4 mb-5 sm:mb-6">
          <div className="flex items-start gap-3 mb-3">
            <Lock size={16} className="text-pulse mt-0.5 shrink-0" />
            <div>
              <span className="text-sm text-light">Set a password</span>
              <p className="text-xs text-ghost mt-1">
                Your wallet is encrypted on this device with this password. There is no way to
                recover it if you forget it — keep your recovery phrase safe as a backup.
              </p>
            </div>
          </div>
          <PasswordFields
            password={password}
            confirmPassword={confirmPassword}
            onPassword={setPassword}
            onConfirmPassword={setConfirmPassword}
          />
        </div>

        <Button onClick={handleCreate} disabled={!confirmed || !showMnemonic || !passwordValid} className="w-full">
          Create Wallet
        </Button>
      </Card>
    </div>
  )
}

function ImportWalletView({ onImport }: { onImport: (mnemonic: string, password?: string) => Promise<void> }) {
  const [seedPhrase, setSeedPhrase] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [isImporting, setIsImporting] = useState(false)
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')

  const wordCount = seedPhrase.trim().split(/\s+/).filter(w => w).length
  // SECURITY (#475): imported wallets are encrypted by default — password required.
  const passwordValid = isPasswordValid(password, confirmPassword)

  const handleImport = async () => {
    setError(null)
    setIsImporting(true)
    try {
      await onImport(seedPhrase, password)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Import failed')
    } finally {
      setIsImporting(false)
    }
  }

  return (
    <div className="max-w-lg mx-auto px-4 sm:px-0">
      <Card className="p-5 sm:p-8">
        <div className="text-center mb-6 sm:mb-8">
          <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
            <KeyRound className="text-pulse" size={28} />
          </div>
          <h2 className="font-display text-xl sm:text-2xl font-bold mb-2">Import Wallet</h2>
          <p className="text-ghost text-sm sm:text-base">Enter your 12 or 24 word recovery phrase to restore your wallet.</p>
        </div>

        <div className="space-y-4">
          <div>
            <label className="block text-sm text-ghost mb-2">Recovery Phrase</label>
            <textarea
              value={seedPhrase}
              onChange={(e) => {
                setSeedPhrase(e.target.value)
                setError(null)
              }}
              placeholder="Enter your recovery phrase, separating each word with a space..."
              rows={4}
              className="w-full p-3 sm:p-4 rounded-lg bg-abyss border border-steel font-mono text-xs sm:text-sm leading-relaxed resize-none focus:outline-none focus:ring-2 focus:ring-pulse/50 focus:border-pulse placeholder:text-ghost/50"
            />
            <div className="flex justify-between items-center mt-2">
              <span className="text-xs text-ghost">
                {wordCount} {wordCount === 1 ? 'word' : 'words'}
              </span>
              {wordCount > 0 && wordCount !== 12 && wordCount !== 24 && (
                <span className="text-xs text-warning">Expected 12 or 24 words</span>
              )}
            </div>
          </div>

          <div className="border-t border-steel pt-4">
            <div className="flex items-start gap-3 mb-3">
              <Lock size={16} className="text-pulse mt-0.5 shrink-0" />
              <div>
                <span className="text-sm text-light">Set a password</span>
                <p className="text-xs text-ghost mt-1">
                  Your wallet is encrypted on this device with this password.
                </p>
              </div>
            </div>
            <PasswordFields
              password={password}
              confirmPassword={confirmPassword}
              onPassword={setPassword}
              onConfirmPassword={setConfirmPassword}
            />
          </div>

          {error && (
            <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
              <AlertCircle size={16} className="shrink-0" />
              <span>{error}</span>
            </div>
          )}

          <Button
            onClick={handleImport}
            disabled={(wordCount !== 12 && wordCount !== 24) || isImporting || !passwordValid}
            className="w-full"
          >
            {isImporting ? (
              <><RefreshCw size={16} className="mr-2 animate-spin" />Importing...</>
            ) : (
              'Import Wallet'
            )}
          </Button>
        </div>
      </Card>
    </div>
  )
}

function WalletDashboard() {
  const { address, balance, transactions, isConnecting, isConnected, refreshBalance, refreshTransactions, resetWallet, send, contacts, searchContacts } = useWallet()

  // Resolve a counterparty address to a saved contact name for the transaction
  // history. We auto-create blank-name "previously paid" entries when sending,
  // so only surface contacts that actually have a non-empty name. Returns
  // `undefined` for unknown/unnamed addresses so the row falls back to the
  // truncated address.
  const resolveName = useMemo(() => {
    const byAddress = new Map(
      contacts
        .filter((c) => c.name.trim().length > 0)
        .map((c) => [c.address.toLowerCase(), c.name] as const)
    )
    return (addr: string): string | undefined => byAddress.get(addr.toLowerCase())
  }, [contacts])
  const { hasFaucet } = useNetwork()
  const [sendOpen, setSendOpen] = useState(false)
  const [sendLinkOpen, setSendLinkOpen] = useState(false)
  const [requestOpen, setRequestOpen] = useState(false)
  const [receiveOpen, setReceiveOpen] = useState(false)
  const [isSending, setIsSending] = useState(false)
  const [showResetConfirm, setShowResetConfirm] = useState(false)

  const handleReset = () => {
    resetWallet()
    setShowResetConfirm(false)
  }

  const handleSend = async (data: SendFormData): Promise<SendResult> => {
    setIsSending(true)
    try {
      // Drive the real client-side send path: derive keys -> scan owned
      // outputs -> build + CLSAG-sign in wasm -> submit to the node. Keys never
      // leave the browser. Returns the node-assigned tx hash.
      const txHash = await send(data.recipient, data.amount, data.memo)
      // Reflect the spend in the UI: refresh balance + history (best effort).
      await Promise.allSettled([refreshBalance(), refreshTransactions()])
      return { success: true, txHash }
    } catch (err) {
      return { success: false, error: err instanceof Error ? err.message : 'Transaction failed' }
    } finally {
      setIsSending(false)
    }
  }

  const estimateFee = async (_amount: bigint, privacyLevel: 'standard' | 'private'): Promise<bigint> => {
    // Estimate based on transaction size
    const sizeBytes = privacyLevel === 'private' ? 22000 : 4000
    // Simple fee calculation: 1 picocredit per byte
    return BigInt(sizeBytes)
  }

  const actionButtons = (
    <>
      <Button onClick={() => setSendOpen(true)}>
        <Send size={16} className="mr-2" />Send
      </Button>
      <Button variant="secondary" onClick={() => setReceiveOpen(true)}>
        <QrCode size={16} className="mr-2" />Receive
      </Button>
      <Button variant="secondary" onClick={() => setSendLinkOpen(true)}>
        <Link2 size={16} className="mr-2" />Send via Link
      </Button>
      <Button variant="secondary" onClick={() => setRequestOpen(true)}>
        <Download size={16} className="mr-2" />Request
      </Button>
      <Link to="/contacts">
        <Button variant="secondary">
          <Users size={16} className="mr-2" />Contacts
        </Button>
      </Link>
      <Button variant="ghost" size="sm" onClick={refreshBalance} disabled={isConnecting}>
        <RefreshCw size={16} className={isConnecting ? 'animate-spin' : ''} />
      </Button>
      <Button variant="ghost" size="sm" onClick={() => setShowResetConfirm(true)} title="Reset wallet">
        <Settings size={16} />
      </Button>
    </>
  )

  return (
    <div className="max-w-4xl mx-auto space-y-4 sm:space-y-6 px-4 sm:px-0">
      <BalanceCard
        balance={balance}
        address={address}
        isLoading={isConnecting}
        isConnected={isConnected}
        isSyncing={isConnecting}
        actions={actionButtons}
      />

      {hasFaucet && <FaucetButton />}

      <OutstandingLinks />

      <TransactionList
        transactions={transactions}
        title="Recent Transactions"
        showChevron={false}
        resolveName={resolveName}
      />

      <SendModal
        isOpen={sendOpen}
        onClose={() => setSendOpen(false)}
        balance={balance}
        estimateFee={estimateFee}
        onSend={handleSend}
        isSending={isSending}
        contacts={contacts}
        onSearchContacts={searchContacts}
      />

      <SendLinkModal isOpen={sendLinkOpen} onClose={() => setSendLinkOpen(false)} />

      <RequestModal isOpen={requestOpen} onClose={() => setRequestOpen(false)} />

      <ReceiveModal
        isOpen={receiveOpen}
        onClose={() => setReceiveOpen(false)}
        onRequestLink={() => setRequestOpen(true)}
      />

      {showResetConfirm && (
        <div className="fixed inset-0 bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4 z-50">
          <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl">
            <div className="text-center mb-5 sm:mb-6">
              <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-danger/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
                <Trash2 className="text-danger" size={28} />
              </div>
              <h3 className="font-display text-lg sm:text-xl font-semibold mb-2">Reset Wallet</h3>
              <p className="text-ghost text-sm">This will remove your wallet from this device. Make sure you have your recovery phrase saved before continuing.</p>
            </div>
            <div className="space-y-3">
              <Button variant="danger" onClick={handleReset} className="w-full justify-center">
                Reset & Start Over
              </Button>
              <Button variant="secondary" onClick={() => setShowResetConfirm(false)} className="w-full justify-center">
                Cancel
              </Button>
            </div>
          </Card>
        </div>
      )}
    </div>
  )
}

function UnlockWalletView({ onUnlock, address }: { onUnlock: (password: string) => Promise<void>; address: string | null }) {
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [isUnlocking, setIsUnlocking] = useState(false)

  const handleUnlock = async () => {
    setError(null)
    setIsUnlocking(true)
    try {
      await onUnlock(password)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unlock failed')
    } finally {
      setIsUnlocking(false)
    }
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && password) {
      handleUnlock()
    }
  }

  return (
    <div className="max-w-lg mx-auto px-4 sm:px-0">
      <Card className="p-5 sm:p-8">
        <div className="text-center mb-6 sm:mb-8">
          <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
            <Lock className="text-pulse" size={28} />
          </div>
          <h2 className="font-display text-xl sm:text-2xl font-bold mb-2">Unlock Wallet</h2>
          <p className="text-ghost text-sm sm:text-base">Enter your password to access your wallet.</p>
          {address && (
            <p className="text-xs text-ghost mt-2 font-mono truncate px-4">{address}</p>
          )}
        </div>

        <div className="space-y-4">
          <Input
            type="password"
            placeholder="Enter password"
            value={password}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
              setPassword(e.target.value)
              setError(null)
            }}
            onKeyDown={handleKeyDown}
            autoFocus
          />

          {error && (
            <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
              <AlertCircle size={16} className="shrink-0" />
              <span>{error}</span>
            </div>
          )}

          <Button
            onClick={handleUnlock}
            disabled={!password || isUnlocking}
            className="w-full"
          >
            {isUnlocking ? (
              <><RefreshCw size={16} className="mr-2 animate-spin" />Unlocking...</>
            ) : (
              'Unlock'
            )}
          </Button>
        </div>
      </Card>
    </div>
  )
}

type WalletMode = 'create' | 'import'

function WalletSetup({ onCreate, onImport }: { onCreate: (mnemonic: string, password?: string) => void; onImport: (mnemonic: string, password?: string) => Promise<void> }) {
  const [mode, setMode] = useState<WalletMode>('create')

  return (
    <div className="space-y-5 sm:space-y-6">
      <div className="max-w-lg mx-auto px-4 sm:px-0">
        <div className="flex rounded-lg bg-abyss border border-steel p-1">
          <button
            onClick={() => setMode('create')}
            className={`flex-1 py-2.5 sm:py-2 px-4 rounded-md text-sm font-medium transition-colors ${
              mode === 'create'
                ? 'bg-steel text-light'
                : 'text-ghost hover:text-light'
            }`}
          >
            Create New
          </button>
          <button
            onClick={() => setMode('import')}
            className={`flex-1 py-2.5 sm:py-2 px-4 rounded-md text-sm font-medium transition-colors ${
              mode === 'import'
                ? 'bg-steel text-light'
                : 'text-ghost hover:text-light'
            }`}
          >
            Import Existing
          </button>
        </div>
      </div>

      {mode === 'create' ? (
        <CreateWalletView onCreate={onCreate} />
      ) : (
        <ImportWalletView onImport={onImport} />
      )}
    </div>
  )
}

export function WalletPage() {
  const { hasWallet: walletExists, isLocked, isConnecting, address, createWallet, importWallet, unlockWallet } = useWallet()

  const handleCreate = async (mnemonic: string, password?: string) => {
    await createWallet(mnemonic, password)
  }

  const handleImport = async (mnemonic: string, password?: string) => {
    await importWallet(mnemonic, password)
  }

  const handleUnlock = async (password: string) => {
    await unlockWallet(password)
  }

  if (isConnecting) {
    return <div className="min-h-screen flex items-center justify-center"><RefreshCw className="animate-spin text-pulse" size={32} /></div>
  }

  // Determine which view to show
  const renderContent = () => {
    if (!walletExists) {
      return <WalletSetup onCreate={handleCreate} onImport={handleImport} />
    }
    if (isLocked) {
      return <UnlockWalletView onUnlock={handleUnlock} address={address} />
    }
    return <WalletDashboard />
  }

  // The marketing landing page lives at `/`, but on the wallet subdomain
  // (wallet.botho.io) `/` redirects back to the wallet (#459). Point the header
  // "back" link at `/home` there so the landing stays reachable from the wallet;
  // on every other host keep `/` so existing nav/e2e behavior is unchanged.
  const homeHref =
    typeof window !== 'undefined' && window.location.hostname.startsWith('wallet.')
      ? '/home'
      : '/'

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to={homeHref} className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">Botho Wallet</span>
            <span className="font-display text-base font-semibold sm:hidden">Wallet</span>
          </Link>
          <NetworkSelector />
        </div>
      </header>
      <main className="py-6 sm:py-8 md:py-12">
        {renderContent()}
      </main>
    </div>
  )
}
