import { useState, useMemo } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, Input, Logo } from '@botho/ui'
import { createMnemonic12 } from '@botho/core'
import { useWallet } from '../contexts/wallet'
import { Wallet, Send, Download, RefreshCw, Copy, Check, ArrowLeft, Shield, Eye, KeyRound, AlertCircle, Lock } from 'lucide-react'

function formatAmount(picocredits: bigint): string {
  const credits = Number(picocredits) / 1e12
  return credits.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 })
}

function CreateWalletView({ onCreate }: { onCreate: (mnemonic: string, password?: string) => void }) {
  const [showMnemonic, setShowMnemonic] = useState(false)
  const [confirmed, setConfirmed] = useState(false)
  const [usePassword, setUsePassword] = useState(false)
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  // Generate mnemonic once and keep it stable
  const mnemonic = useMemo(() => createMnemonic12(), [])

  const passwordsMatch = password === confirmPassword
  const passwordValid = !usePassword || (password.length >= 4 && passwordsMatch)

  const handleCreate = () => {
    onCreate(mnemonic, usePassword ? password : undefined)
  }

  return (
    <div className="max-w-lg mx-auto">
      <Card className="p-8">
        <div className="text-center mb-8">
          <div className="w-16 h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-4">
            <Shield className="text-pulse" size={32} />
          </div>
          <h2 className="font-display text-2xl font-bold mb-2">Create New Wallet</h2>
          <p className="text-ghost">Write down your recovery phrase and store it safely.</p>
        </div>

        <div className="relative mb-6">
          <div className={`p-4 rounded-lg bg-abyss border border-steel font-mono text-sm leading-relaxed ${showMnemonic ? '' : 'blur-sm select-none'}`}>
            {mnemonic}
          </div>
          {!showMnemonic && (
            <button
              onClick={() => setShowMnemonic(true)}
              className="absolute inset-0 flex items-center justify-center gap-2 text-ghost hover:text-light transition-colors"
            >
              <Eye size={20} />
              Click to reveal
            </button>
          )}
        </div>

        <label className="flex items-start gap-3 mb-4 cursor-pointer">
          <input type="checkbox" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} className="mt-1" />
          <span className="text-sm text-ghost">I have written down my recovery phrase and stored it in a safe place.</span>
        </label>

        <div className="border-t border-steel pt-4 mb-6">
          <label className="flex items-start gap-3 cursor-pointer">
            <input type="checkbox" checked={usePassword} onChange={(e) => setUsePassword(e.target.checked)} className="mt-1" />
            <div>
              <span className="text-sm text-light">Protect with password</span>
              <p className="text-xs text-ghost mt-1">Add a password to encrypt your wallet in this browser. You'll need to enter it each time you open the wallet.</p>
            </div>
          </label>

          {usePassword && (
            <div className="mt-4 space-y-3">
              <div>
                <Input
                  type="password"
                  placeholder="Password (min 4 characters)"
                  value={password}
                  onChange={(e: React.ChangeEvent<HTMLInputElement>) => setPassword(e.target.value)}
                />
              </div>
              <div>
                <Input
                  type="password"
                  placeholder="Confirm password"
                  value={confirmPassword}
                  onChange={(e: React.ChangeEvent<HTMLInputElement>) => setConfirmPassword(e.target.value)}
                />
                {confirmPassword && !passwordsMatch && (
                  <p className="text-xs text-danger mt-1">Passwords don't match</p>
                )}
              </div>
            </div>
          )}
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
  const [usePassword, setUsePassword] = useState(false)
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')

  const wordCount = seedPhrase.trim().split(/\s+/).filter(w => w).length
  const passwordsMatch = password === confirmPassword
  const passwordValid = !usePassword || (password.length >= 4 && passwordsMatch)

  const handleImport = async () => {
    setError(null)
    setIsImporting(true)
    try {
      await onImport(seedPhrase, usePassword ? password : undefined)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Import failed')
    } finally {
      setIsImporting(false)
    }
  }

  return (
    <div className="max-w-lg mx-auto">
      <Card className="p-8">
        <div className="text-center mb-8">
          <div className="w-16 h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-4">
            <KeyRound className="text-pulse" size={32} />
          </div>
          <h2 className="font-display text-2xl font-bold mb-2">Import Wallet</h2>
          <p className="text-ghost">Enter your 12 or 24 word recovery phrase to restore your wallet.</p>
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
              className="w-full p-4 rounded-lg bg-abyss border border-steel font-mono text-sm leading-relaxed resize-none focus:outline-none focus:ring-2 focus:ring-pulse/50 focus:border-pulse placeholder:text-ghost/50"
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
            <label className="flex items-start gap-3 cursor-pointer">
              <input type="checkbox" checked={usePassword} onChange={(e) => setUsePassword(e.target.checked)} className="mt-1" />
              <div>
                <span className="text-sm text-light">Protect with password</span>
                <p className="text-xs text-ghost mt-1">Encrypt your wallet in this browser.</p>
              </div>
            </label>

            {usePassword && (
              <div className="mt-4 space-y-3">
                <Input
                  type="password"
                  placeholder="Password (min 4 characters)"
                  value={password}
                  onChange={(e: React.ChangeEvent<HTMLInputElement>) => setPassword(e.target.value)}
                />
                <div>
                  <Input
                    type="password"
                    placeholder="Confirm password"
                    value={confirmPassword}
                    onChange={(e: React.ChangeEvent<HTMLInputElement>) => setConfirmPassword(e.target.value)}
                  />
                  {confirmPassword && !passwordsMatch && (
                    <p className="text-xs text-danger mt-1">Passwords don't match</p>
                  )}
                </div>
              </div>
            )}
          </div>

          {error && (
            <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
              <AlertCircle size={16} />
              {error}
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
  const { address, balance, transactions, isConnecting, refreshBalance } = useWallet()
  const [copied, setCopied] = useState(false)
  const [sendOpen, setSendOpen] = useState(false)
  const [receiveOpen, setReceiveOpen] = useState(false)

  const copyAddress = async () => {
    if (address) {
      await navigator.clipboard.writeText(address)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    }
  }

  return (
    <div className="max-w-4xl mx-auto space-y-6">
      <Card className="p-8">
        <div className="flex items-start justify-between mb-6">
          <div>
            <p className="text-ghost text-sm mb-1">Total Balance</p>
            <h2 className="font-display text-4xl font-bold">
              {isConnecting ? (
                <span className="shimmer inline-block w-48 h-10 rounded" />
              ) : (
                <>{formatAmount(balance?.total ?? 0n)} <span className="text-xl text-ghost">BTH</span></>
              )}
            </h2>
          </div>
          <Button variant="ghost" size="sm" onClick={refreshBalance} disabled={isConnecting}>
            <RefreshCw size={16} className={isConnecting ? 'animate-spin' : ''} />
          </Button>
        </div>

        <div className="flex items-center gap-2 p-3 rounded-lg bg-abyss border border-steel">
          <span className="text-ghost text-sm truncate flex-1 font-mono">{address ?? 'Loading...'}</span>
          <button onClick={copyAddress} className="p-2 text-ghost hover:text-light transition-colors">
            {copied ? <Check size={16} className="text-success" /> : <Copy size={16} />}
          </button>
        </div>

        <div className="flex gap-4 mt-6">
          <Button onClick={() => setSendOpen(true)} className="flex-1"><Send size={16} className="mr-2" />Send</Button>
          <Button variant="secondary" onClick={() => setReceiveOpen(true)} className="flex-1"><Download size={16} className="mr-2" />Receive</Button>
        </div>
      </Card>

      <Card className="p-6">
        <h3 className="font-display text-lg font-semibold mb-4">Recent Transactions</h3>
        {transactions.length === 0 ? (
          <div className="text-center py-12 text-ghost">
            <Wallet size={48} className="mx-auto mb-4 opacity-50" />
            <p>No transactions yet</p>
          </div>
        ) : (
          <div className="space-y-3">
            {transactions.map((tx) => (
              <div key={tx.id} className="flex items-center justify-between p-4 rounded-lg bg-abyss border border-steel">
                <div className="flex items-center gap-3">
                  <div className={`w-10 h-10 rounded-full flex items-center justify-center ${tx.type === 'receive' ? 'bg-success/10 text-success' : 'bg-danger/10 text-danger'}`}>
                    {tx.type === 'receive' ? <Download size={18} /> : <Send size={18} />}
                  </div>
                  <div>
                    <p className="font-medium capitalize">{tx.type}</p>
                    <p className="text-sm text-ghost">{tx.confirmations} confirmations</p>
                  </div>
                </div>
                <p className={`font-mono font-medium ${tx.type === 'send' ? 'text-danger' : 'text-success'}`}>
                  {tx.type === 'send' ? '-' : '+'}{formatAmount(tx.amount)} BTH
                </p>
              </div>
            ))}
          </div>
        )}
      </Card>

      {sendOpen && (
        <div className="fixed inset-0 bg-void/80 backdrop-blur-sm flex items-center justify-center p-4 z-50">
          <Card className="w-full max-w-md p-6">
            <h3 className="font-display text-xl font-semibold mb-6">Send BTH</h3>
            <div className="space-y-4">
              <div><label className="block text-sm text-ghost mb-2">Recipient Address</label><Input placeholder="botho://1/..." /></div>
              <div><label className="block text-sm text-ghost mb-2">Amount</label><Input type="number" placeholder="0.00" /></div>
              <div className="flex gap-3 mt-6">
                <Button variant="secondary" onClick={() => setSendOpen(false)} className="flex-1">Cancel</Button>
                <Button className="flex-1">Send</Button>
              </div>
            </div>
          </Card>
        </div>
      )}

      {receiveOpen && (
        <div className="fixed inset-0 bg-void/80 backdrop-blur-sm flex items-center justify-center p-4 z-50">
          <Card className="w-full max-w-md p-6">
            <div className="text-center mb-6">
              <div className="w-16 h-16 rounded-full bg-success/10 flex items-center justify-center mx-auto mb-4">
                <Download className="text-success" size={32} />
              </div>
              <h3 className="font-display text-xl font-semibold mb-2">Receive BTH</h3>
              <p className="text-ghost text-sm">Share your address to receive payments</p>
            </div>
            <div className="space-y-4">
              <div>
                <label className="block text-sm text-ghost mb-2">Your Wallet Address</label>
                <div className="p-4 rounded-lg bg-abyss border border-steel font-mono text-sm break-all select-all">
                  {address ?? 'Loading...'}
                </div>
              </div>
              <Button
                onClick={() => {
                  copyAddress()
                  setTimeout(() => setReceiveOpen(false), 1500)
                }}
                className="w-full"
              >
                {copied ? <><Check size={16} className="mr-2" />Copied!</> : <><Copy size={16} className="mr-2" />Copy Address</>}
              </Button>
              <Button variant="secondary" onClick={() => setReceiveOpen(false)} className="w-full">
                Close
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
    <div className="max-w-lg mx-auto">
      <Card className="p-8">
        <div className="text-center mb-8">
          <div className="w-16 h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-4">
            <Lock className="text-pulse" size={32} />
          </div>
          <h2 className="font-display text-2xl font-bold mb-2">Unlock Wallet</h2>
          <p className="text-ghost">Enter your password to access your wallet.</p>
          {address && (
            <p className="text-xs text-ghost mt-2 font-mono truncate">{address}</p>
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
              <AlertCircle size={16} />
              {error}
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
    <div className="space-y-6">
      <div className="max-w-lg mx-auto">
        <div className="flex rounded-lg bg-abyss border border-steel p-1">
          <button
            onClick={() => setMode('create')}
            className={`flex-1 py-2 px-4 rounded-md text-sm font-medium transition-colors ${
              mode === 'create'
                ? 'bg-steel text-light'
                : 'text-ghost hover:text-light'
            }`}
          >
            Create New
          </button>
          <button
            onClick={() => setMode('import')}
            className={`flex-1 py-2 px-4 rounded-md text-sm font-medium transition-colors ${
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

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-6 py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-3">
            <ArrowLeft size={20} className="text-ghost" />
            <Logo size="md" showText={false} />
            <span className="font-display text-lg font-semibold">Botho Wallet</span>
          </Link>
        </div>
      </header>
      <main className="py-12 px-6">
        {renderContent()}
      </main>
    </div>
  )
}
