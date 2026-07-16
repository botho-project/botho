import { useState, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { Button, Card, Input, Logo, ModalOverlay } from '@botho/ui'
import { createMnemonic12 } from '@botho/core'
import { BalanceCard, TransactionList, SendModal, type SendFormData, type SendResult } from '@botho/features'
import { useWallet } from '../contexts/wallet'
import { useNetwork } from '../contexts/network'
import { NetworkSelector } from '../components/NetworkSelector'
import { LocaleSwitcher } from '../components/LocaleSwitcher'
import { FaucetButton } from '../components/FaucetButton'
import { SendLinkModal } from '../components/SendLinkModal'
import { RequestModal } from '../components/RequestModal'
import { ReceiveModal } from '../components/ReceiveModal'
import { OutstandingLinks } from '../components/OutstandingLinks'
import { OfflineBanner } from '../components/OfflineBanner'
import { CustomRpcTrustGate, CustomNodeBanner } from '../components/CustomRpcTrustGate'
import { PasswordFields, PasswordSettingsModal, isPasswordValid } from '../components/PasswordSettingsModal'
import { Send, Link2, Download, RefreshCw, ArrowLeft, Shield, ShieldCheck, Eye, KeyRound, AlertCircle, Lock, Settings, Trash2, Users, QrCode, Clock } from 'lucide-react'

function CreateWalletView({ onCreate }: { onCreate: (mnemonic: string, password?: string) => void }) {
  const { t } = useTranslation('wallet')
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
          <h2 className="font-display text-xl sm:text-2xl font-bold mb-2">{t('createView.title')}</h2>
          <p className="text-ghost text-sm sm:text-base">{t('createView.subtitle')}</p>
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
              <span className="text-sm">{t('createView.clickToReveal')}</span>
            </button>
          )}
        </div>

        <label className="flex items-start gap-3 mb-4 cursor-pointer">
          <input type="checkbox" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} className="mt-1 w-4 h-4 accent-pulse" />
          <span className="text-sm text-ghost">
            {t('createView.confirmPhrase')}
            <span className="text-danger ml-1">*</span>
          </span>
        </label>

        <div className="border-t border-steel pt-4 mb-5 sm:mb-6">
          <div className="flex items-start gap-3 mb-3">
            <Lock size={16} className="text-pulse mt-0.5 shrink-0" />
            <div>
              <span className="text-sm text-light">{t('createView.setPassword')}</span>
              <p className="text-xs text-ghost mt-1">
                {t('createView.passwordNote')}
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
          {t('createView.createButton')}
        </Button>
      </Card>
    </div>
  )
}

function ImportWalletView({ onImport }: { onImport: (mnemonic: string, password?: string) => Promise<void> }) {
  const { t } = useTranslation('wallet')
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
      setError(err instanceof Error ? err.message : t('importView.importFailed'))
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
          <h2 className="font-display text-xl sm:text-2xl font-bold mb-2">{t('importView.title')}</h2>
          <p className="text-ghost text-sm sm:text-base">{t('importView.subtitle')}</p>
        </div>

        <div className="space-y-4">
          <div>
            <label className="block text-sm text-ghost mb-2">{t('importView.recoveryPhraseLabel')}</label>
            <textarea
              value={seedPhrase}
              onChange={(e) => {
                setSeedPhrase(e.target.value)
                setError(null)
              }}
              placeholder={t('importView.placeholder')}
              rows={4}
              className="w-full p-3 sm:p-4 rounded-lg bg-abyss border border-steel font-mono text-xs sm:text-sm leading-relaxed resize-none focus:outline-none focus:ring-2 focus:ring-pulse/50 focus:border-pulse placeholder:text-ghost/50"
            />
            <div className="flex justify-between items-center mt-2">
              <span className="text-xs text-ghost">
                {wordCount === 1
                  ? t('importView.wordCountOne', { count: wordCount })
                  : t('importView.wordCountOther', { count: wordCount })}
              </span>
              {wordCount > 0 && wordCount !== 12 && wordCount !== 24 && (
                <span className="text-xs text-warning">{t('importView.expectedWords')}</span>
              )}
            </div>
          </div>

          <div className="border-t border-steel pt-4">
            <div className="flex items-start gap-3 mb-3">
              <Lock size={16} className="text-pulse mt-0.5 shrink-0" />
              <div>
                <span className="text-sm text-light">{t('importView.setPassword')}</span>
                <p className="text-xs text-ghost mt-1">
                  {t('importView.passwordNote')}
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
              <><RefreshCw size={16} className="mr-2 animate-spin" />{t('importView.importing')}</>
            ) : (
              t('importView.importButton')
            )}
          </Button>
        </div>
      </Card>
    </div>
  )
}

/**
 * Auto-lock timeout options for the settings control (#490). 0 = Off/Never.
 * Labels are resolved via i18n (`wallet.autoLock.*`) at render time.
 */
const AUTO_LOCK_OPTIONS: Array<{ minutes: number; labelKey: string }> = [
  { minutes: 1, labelKey: 'autoLock.minute' },
  { minutes: 5, labelKey: 'autoLock.fiveMinutes' },
  { minutes: 15, labelKey: 'autoLock.fifteenMinutes' },
  { minutes: 30, labelKey: 'autoLock.thirtyMinutes' },
  { minutes: 60, labelKey: 'autoLock.hour' },
  { minutes: 0, labelKey: 'autoLock.off' },
]

/**
 * Settings sheet (#490): wallet security controls — auto-lock timeout, Lock now,
 * and reset. Lock + auto-lock are only meaningful for an ENCRYPTED wallet (a
 * plaintext wallet has no password to unlock with), so for a plaintext wallet
 * the Lock control is disabled and routes the user to set a password first.
 */
function SettingsModal({
  isEncrypted,
  autoLockMinutes,
  onAutoLockChange,
  onLock,
  onSetPassword,
  onReset,
  onClose,
}: {
  isEncrypted: boolean
  autoLockMinutes: number
  onAutoLockChange: (minutes: number) => void
  onLock: () => void
  onSetPassword: () => void
  onReset: () => void
  onClose: () => void
}) {
  const { t } = useTranslation('wallet')
  return (
    // Shared dismissal policy (#655): backdrop click / Escape close, same as
    // the explicit Close button.
    <ModalOverlay
      onDismiss={onClose}
      className="bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4"
    >
      <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl">
        <div className="text-center mb-5 sm:mb-6">
          <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
            <Settings className="text-pulse" size={28} />
          </div>
          <h3 className="font-display text-lg sm:text-xl font-semibold mb-2">{t('settingsModal.title')}</h3>
          <p className="text-ghost text-sm">{t('settingsModal.subtitle')}</p>
        </div>

        <div className="space-y-5">
          {/* Auto-lock timeout */}
          <div>
            <div className="flex items-center gap-2 mb-2">
              <Clock size={16} className="text-pulse shrink-0" />
              <span className="text-sm font-medium text-light">{t('settingsModal.autoLockLabel')}</span>
            </div>
            <select
              value={autoLockMinutes}
              onChange={(e) => onAutoLockChange(Number(e.target.value))}
              disabled={!isEncrypted}
              className="w-full p-2.5 rounded-lg bg-abyss border border-steel text-sm text-light focus:outline-none focus:ring-2 focus:ring-pulse/50 focus:border-pulse disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {AUTO_LOCK_OPTIONS.map((opt) => (
                <option key={opt.minutes} value={opt.minutes}>
                  {t(opt.labelKey)}
                </option>
              ))}
            </select>
            <p className="text-xs text-ghost mt-1">
              {isEncrypted
                ? t('settingsModal.autoLockEnabled')
                : t('settingsModal.autoLockDisabled')}
            </p>
          </div>

          {/* Lock now */}
          <div>
            <Button
              variant="secondary"
              onClick={onLock}
              disabled={!isEncrypted}
              className="w-full justify-center"
              title={isEncrypted ? t('settingsModal.lockWalletTitle') : t('settingsModal.enableLockingTitle')}
            >
              <Lock size={16} className="mr-2" />
              {t('settingsModal.lockWallet')}
            </Button>
            {!isEncrypted && (
              <button
                type="button"
                onClick={onSetPassword}
                className="text-xs text-pulse hover:underline mt-2"
              >
                {t('settingsModal.enableLockingLink')}
              </button>
            )}
          </div>

          {/* Reset */}
          <div className="border-t border-steel pt-4">
            <Button variant="danger" onClick={onReset} className="w-full justify-center">
              <Trash2 size={16} className="mr-2" />
              {t('settingsModal.resetWallet')}
            </Button>
          </div>

          <Button variant="ghost" onClick={onClose} className="w-full justify-center">
            {t('settingsModal.close')}
          </Button>
        </div>
      </Card>
    </ModalOverlay>
  )
}

function WalletDashboard() {
  const { t } = useTranslation('wallet')
  const { address, balance, transactions, isConnecting, isConnected, refreshBalance, refreshTransactions, resetWallet, send, estimateFee, contacts, searchContacts, isEncrypted, setPassword, changePassword, lockWallet, autoLockMinutes, setAutoLockMinutes } = useWallet()

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
  const [showPasswordModal, setShowPasswordModal] = useState(false)
  const [showSettings, setShowSettings] = useState(false)

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
      return { success: false, error: err instanceof Error ? err.message : t('dashboard.transactionFailed') }
    } finally {
      setIsSending(false)
    }
  }

  // The real fee estimate (with the node-computed cluster factor) comes from the
  // wallet context, which mirrors the send flow's cluster-wealth derivation
  // (#635). The SendModal's `estimateFee` prop ignores the privacy-level second
  // argument, so a thin wrapper adapts the context's `(amount) => FeeEstimate`
  // signature to the modal's `(amount, privacyLevel) => FeeEstimate`.
  const handleEstimateFee = (amount: bigint, _privacyLevel: 'standard' | 'private') =>
    estimateFee(amount)

  const actionButtons = (
    <>
      <Button onClick={() => setSendOpen(true)}>
        <Send size={16} className="mr-2" />{t('dashboard.send')}
      </Button>
      <Button variant="secondary" onClick={() => setReceiveOpen(true)}>
        <QrCode size={16} className="mr-2" />{t('dashboard.receive')}
      </Button>
      <Button variant="secondary" onClick={() => setSendLinkOpen(true)}>
        <Link2 size={16} className="mr-2" />{t('dashboard.sendViaLink')}
      </Button>
      <Button variant="secondary" onClick={() => setRequestOpen(true)}>
        <Download size={16} className="mr-2" />{t('dashboard.request')}
      </Button>
      <Link to="/contacts">
        <Button variant="secondary">
          <Users size={16} className="mr-2" />{t('dashboard.contacts')}
        </Button>
      </Link>
      <Button variant="ghost" size="sm" onClick={refreshBalance} disabled={isConnecting}>
        <RefreshCw size={16} className={isConnecting ? 'animate-spin' : ''} />
      </Button>
      <Button
        variant="ghost"
        size="sm"
        onClick={lockWallet}
        disabled={!isEncrypted}
        title={isEncrypted ? t('dashboard.lockWalletTitle') : t('dashboard.enableLockingTitle')}
      >
        <Lock size={16} />
      </Button>
      <Button variant="ghost" size="sm" onClick={() => setShowSettings(true)} title={t('dashboard.settingsTitle')}>
        <Settings size={16} />
      </Button>
    </>
  )

  return (
    <div className="max-w-4xl mx-auto space-y-4 sm:space-y-6 px-4 sm:px-0">
      {/* Active-node-offline prompt (#492): surfaces when the selected ingress
          node goes unreachable mid-use, with a one-click switch action. */}
      <OfflineBanner />

      {/* Custom-node-from-a-link banner (#587): reminds the user they accepted a
          `?rpc=` deep link and are off the default seeds, with a one-tap revert. */}
      <CustomNodeBanner />

      <BalanceCard
        balance={balance}
        address={address}
        isLoading={isConnecting}
        isConnected={isConnected}
        isSyncing={isConnecting}
        actions={actionButtons}
      />

      {hasFaucet && <FaucetButton />}

      <Card className="p-4 sm:p-5">
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <div className="flex items-start gap-3">
            <Shield size={18} className={isEncrypted ? 'text-success mt-0.5 shrink-0' : 'text-warning mt-0.5 shrink-0'} />
            <div>
              <p className="text-sm font-medium text-light">
                {isEncrypted ? t('dashboard.protectedTitle') : t('dashboard.unprotectedTitle')}
              </p>
              <p className="text-xs text-ghost mt-1">
                {isEncrypted
                  ? t('dashboard.protectedBody')
                  : t('dashboard.unprotectedBody')}
              </p>
            </div>
          </div>
          <Button
            variant={isEncrypted ? 'secondary' : 'primary'}
            size="sm"
            onClick={() => setShowPasswordModal(true)}
          >
            <KeyRound size={16} className="mr-2" />
            {isEncrypted ? t('dashboard.changePassword') : t('dashboard.setPassword')}
          </Button>
        </div>
      </Card>

      <OutstandingLinks />

      <TransactionList
        transactions={transactions}
        title={t('dashboard.recentTransactions')}
        showChevron={false}
        resolveName={resolveName}
      />

      <SecurityModelFooter isEncrypted={isEncrypted} />

      <SendModal
        isOpen={sendOpen}
        onClose={() => setSendOpen(false)}
        balance={balance}
        estimateFee={handleEstimateFee}
        onSend={handleSend}
        isSending={isSending}
        contacts={contacts}
        onSearchContacts={searchContacts}
        ownAddress={address}
      />

      <SendLinkModal isOpen={sendLinkOpen} onClose={() => setSendLinkOpen(false)} />

      <RequestModal isOpen={requestOpen} onClose={() => setRequestOpen(false)} />

      <ReceiveModal
        isOpen={receiveOpen}
        onClose={() => setReceiveOpen(false)}
        onRequestLink={() => setRequestOpen(true)}
      />

      {showSettings && (
        <SettingsModal
          isEncrypted={isEncrypted}
          autoLockMinutes={autoLockMinutes}
          onAutoLockChange={setAutoLockMinutes}
          onLock={() => {
            setShowSettings(false)
            lockWallet()
          }}
          onSetPassword={() => {
            setShowSettings(false)
            setShowPasswordModal(true)
          }}
          onReset={() => {
            setShowSettings(false)
            setShowResetConfirm(true)
          }}
          onClose={() => setShowSettings(false)}
        />
      )}

      {showPasswordModal && (
        <PasswordSettingsModal
          mode={isEncrypted ? 'change' : 'set'}
          onClose={() => setShowPasswordModal(false)}
          onSetPassword={setPassword}
          onChangePassword={changePassword}
        />
      )}

      {showResetConfirm && (
        // Destructive confirm (#655): backdrop click / Escape dismiss to the
        // safe Cancel path — never to the destructive Reset action.
        <ModalOverlay
          onDismiss={() => setShowResetConfirm(false)}
          className="bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4"
        >
          <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl">
            <div className="text-center mb-5 sm:mb-6">
              <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-danger/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
                <Trash2 className="text-danger" size={28} />
              </div>
              <h3 className="font-display text-lg sm:text-xl font-semibold mb-2">{t('resetModal.title')}</h3>
              <p className="text-ghost text-sm">{t('resetModal.body')}</p>
            </div>
            <div className="space-y-3">
              <Button variant="danger" onClick={handleReset} className="w-full justify-center">
                {t('resetModal.confirm')}
              </Button>
              <Button variant="secondary" onClick={() => setShowResetConfirm(false)} className="w-full justify-center">
                {t('resetModal.cancel')}
              </Button>
            </div>
          </Card>
        </ModalOverlay>
      )}
    </div>
  )
}

/**
 * Bottom-of-dashboard explainer for the web wallet's privacy and security
 * model. Every claim here must stay true to the implementation: client-side
 * key generation + WASM signing (`@botho/core` wallet + bth_wasm_signer),
 * full-chain output download with local trial-scanning (RemoteNodeAdapter
 * `chain_getOutputs` — the node never learns which outputs are yours), and
 * the AES-256-GCM / PBKDF2-SHA256@600k vault when a password is set.
 */
function SecurityModelFooter({ isEncrypted }: { isEncrypted: boolean }) {
  return (
    <Card className="p-5 sm:p-6 mt-2">
      <div className="flex items-center gap-2 mb-3">
        <ShieldCheck className="text-pulse" size={18} />
        <h3 className="font-display text-sm font-semibold tracking-wide uppercase text-light">
          How this wallet protects you
        </h3>
      </div>
      <ul className="space-y-2 text-sm text-ghost">
        <li>
          <span className="text-light">Your keys never leave this browser.</span>{' '}
          The recovery phrase is generated locally and transactions are signed
          client-side; the node you connect to only ever sees signed
          transactions, never keys.
        </li>
        <li>
          <span className="text-light">The node can't see what's yours.</span>{' '}
          The wallet downloads the chain's outputs and scans them locally with
          your view key. Thanks to stealth addresses, the node learns your IP
          and that you're syncing — not your balance, your addresses, or which
          payments are yours.
        </li>
        <li>
          <span className="text-light">
            {isEncrypted ? 'Encrypted at rest.' : 'Set a password to encrypt at rest.'}
          </span>{' '}
          {isEncrypted
            ? 'Your wallet is stored as an AES-256-GCM vault; the key is derived from your password (PBKDF2, 600k iterations) and is never stored.'
            : 'Without a password, the wallet is stored unencrypted in this browser profile — anyone with access to this device can open it.'}
        </li>
        <li>
          <span className="text-light">What this can't protect against:</span>{' '}
          malware or browser extensions that can read this page, someone with
          your recovery phrase, or a compromised device. For the strongest
          setup, run your own node and keep the phrase on paper only.
        </li>
      </ul>
      <p className="text-xs text-dim mt-3">
        Details in the{' '}
        <Link to="/docs#privacy" className="text-pulse hover:underline">
          privacy documentation
        </Link>
        .
      </p>
    </Card>
  )
}

function UnlockWalletView({ onUnlock, address }: { onUnlock: (password: string) => Promise<void>; address: string | null }) {
  const { t } = useTranslation('wallet')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [isUnlocking, setIsUnlocking] = useState(false)

  const handleUnlock = async () => {
    setError(null)
    setIsUnlocking(true)
    try {
      await onUnlock(password)
    } catch (err) {
      setError(err instanceof Error ? err.message : t('unlockView.unlockFailed'))
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
          <h2 className="font-display text-xl sm:text-2xl font-bold mb-2">{t('unlockView.title')}</h2>
          <p className="text-ghost text-sm sm:text-base">{t('unlockView.subtitle')}</p>
          {address && (
            <p className="text-xs text-ghost mt-2 font-mono truncate px-4">{address}</p>
          )}
        </div>

        <div className="space-y-4">
          <Input
            type="password"
            placeholder={t('unlockView.passwordPlaceholder')}
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
              <><RefreshCw size={16} className="mr-2 animate-spin" />{t('unlockView.unlocking')}</>
            ) : (
              t('unlockView.unlock')
            )}
          </Button>
        </div>
      </Card>
    </div>
  )
}

type WalletMode = 'create' | 'import'

function WalletSetup({ onCreate, onImport }: { onCreate: (mnemonic: string, password?: string) => void; onImport: (mnemonic: string, password?: string) => Promise<void> }) {
  const { t } = useTranslation('wallet')
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
            {t('setup.createNew')}
          </button>
          <button
            onClick={() => setMode('import')}
            className={`flex-1 py-2.5 sm:py-2 px-4 rounded-md text-sm font-medium transition-colors ${
              mode === 'import'
                ? 'bg-steel text-light'
                : 'text-ghost hover:text-light'
            }`}
          >
            {t('setup.importExisting')}
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
  const { t } = useTranslation('wallet')
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
      {/* Custom-RPC deep-link trust gate (#587): a `?rpc=` link is surfaced as a
          pending prompt and must be explicitly accepted before it can switch the
          active node. Rendered at the page root so it overlays any wallet view. */}
      <CustomRpcTrustGate />
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to={homeHref} className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">{t('header.walletNameLong')}</span>
            <span className="font-display text-base font-semibold sm:hidden">{t('header.walletNameShort')}</span>
          </Link>
          <div className="flex items-center gap-3">
            {/* wBTH discovery entry point on the wallet host (#1030). */}
            <Link
              to="/trade"
              className="text-sm text-ghost hover:text-light transition-colors whitespace-nowrap hidden sm:inline"
            >
              {t('bridge:nav.trade')}
            </Link>
            <LocaleSwitcher className="whitespace-nowrap" />
            <NetworkSelector />
          </div>
        </div>
      </header>
      <main className="py-6 sm:py-8 md:py-12">
        {renderContent()}
      </main>
    </div>
  )
}
