import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, Input, Logo } from '@botho/ui'
import { formatBTH, parseBTH, isValidAddress, createMnemonic12 } from '@botho/core'
import {
  Send,
  AlertCircle,
  Check,
  Loader2,
  ArrowLeft,
  Lock,
  Eye,
  ExternalLink,
  ShieldAlert,
} from 'lucide-react'
import { useWallet } from '../contexts/wallet'
import { useNetwork } from '../contexts/network'
import { parsePaymentRequestFragment, type PaymentRequest } from '../lib/payment-request'

/**
 * Pay page for payment-request links (#470) — the *pull* complement to the
 * claim page (#460, `claim.tsx`).
 *
 * Reads the requester's PUBLIC address + optional amount/memo from the URL
 * FRAGMENT (never sent to a server), strips it from the address bar, and — if a
 * wallet is unlocked — pre-fills a send to the requester. The payer confirms and
 * pays via the existing `send()` path. Unlike a claim link there is no secret:
 * this link cannot move anyone's money; it only suggests a payment.
 *
 * If there's no wallet (or it's locked), the page prompts the visitor to
 * create / import / unlock first, then resumes the pay flow with the parsed
 * request preserved in component state (the fragment is already stripped).
 */

type ParseState = 'parsing' | 'invalid' | 'ready'
type SetupMode = 'unlock' | 'create' | 'import'

export function PayPage() {
  const {
    hasWallet,
    isLocked,
    address,
    getContactName,
    send,
    refreshBalance,
    refreshTransactions,
    createWallet,
    importWallet,
    unlockWallet,
    balance,
  } = useWallet()
  const { network } = useNetwork()

  const [parseState, setParseState] = useState<ParseState>('parsing')
  const [request, setRequest] = useState<PaymentRequest | null>(null)
  const [parseError, setParseError] = useState<string | null>(null)

  // 1. Parse the fragment ONCE on mount, then strip it from the URL so the
  //    requester's address does not linger in the address bar / history / logs.
  useEffect(() => {
    const hash = window.location.hash
    if (!hash || hash === '#') {
      setParseState('invalid')
      setParseError('No payment request found. The link should look like .../pay#…')
      return
    }
    try {
      const parsed = parsePaymentRequestFragment(hash)
      setRequest(parsed)
      try {
        window.history.replaceState(null, '', window.location.pathname + window.location.search)
      } catch {
        // replaceState may be unavailable in some embeds; non-fatal.
      }
      setParseState('ready')
    } catch (err) {
      setParseState('invalid')
      setParseError(err instanceof Error ? err.message : 'This payment-request link is not valid.')
    }
  }, [])

  const explorerBase = network.explorerUrl

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">
              Botho Wallet
            </span>
          </Link>
        </div>
      </header>

      <main className="py-8 sm:py-12">
        <div className="max-w-lg mx-auto px-4 sm:px-0">
          <Card className="p-5 sm:p-8">
            <div className="text-center mb-6">
              <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
                <Send className="text-pulse" size={26} />
              </div>
              <h2 className="font-display text-xl sm:text-2xl font-bold mb-2">Send a Payment</h2>
            </div>

            {parseState === 'parsing' && (
              <div className="flex flex-col items-center gap-3 py-6 text-ghost">
                <Loader2 size={28} className="animate-spin text-pulse" />
                <p className="text-sm">Reading the payment request…</p>
              </div>
            )}

            {parseState === 'invalid' && (
              <div className="flex items-start gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
                <AlertCircle size={16} className="shrink-0 mt-0.5" />
                <span>{parseError ?? 'This payment-request link is not valid.'}</span>
              </div>
            )}

            {parseState === 'ready' && request && (
              <>
                {!hasWallet || isLocked ? (
                  <WalletGate
                    isLocked={isLocked}
                    onCreate={createWallet}
                    onImport={importWallet}
                    onUnlock={unlockWallet}
                  />
                ) : (
                  <PayConfirm
                    request={request}
                    ownAddress={address}
                    balance={balance}
                    getContactName={getContactName}
                    send={send}
                    refreshBalance={refreshBalance}
                    refreshTransactions={refreshTransactions}
                    explorerBase={explorerBase}
                  />
                )}
              </>
            )}
          </Card>
        </div>
      </main>
    </div>
  )
}

/**
 * The actual pay confirmation, shown once a wallet is unlocked. Pre-fills the
 * recipient (from the request) and amount (editable, defaulting to the requested
 * amount), and pays via the existing `send()` path.
 */
function PayConfirm({
  request,
  ownAddress,
  balance,
  getContactName,
  send,
  refreshBalance,
  refreshTransactions,
  explorerBase,
}: {
  request: PaymentRequest
  ownAddress: string | null
  balance: import('@botho/core').Balance | null
  getContactName: (address: string) => string
  send: (to: string, amount: bigint, memo?: string) => Promise<string>
  refreshBalance: () => Promise<void>
  refreshTransactions: () => Promise<void>
  explorerBase?: string
}) {
  const [amountStr, setAmountStr] = useState(
    request.amount !== undefined ? formatBTH(request.amount, { separators: false }) : '',
  )
  const [isSending, setIsSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [txHash, setTxHash] = useState<string | null>(null)

  const contactName = useMemo(() => {
    const name = getContactName(request.to)
    // getContactName falls back to a shortened address when unknown; only show a
    // "name" when it differs from the raw address (i.e. an actual contact).
    return name && name !== request.to ? name : null
  }, [getContactName, request.to])

  const addressValid = isValidAddress(request.to)
  const isSelfPay = ownAddress != null && ownAddress === request.to

  let amount = 0n
  let amountError: string | null = null
  if (amountStr.trim()) {
    try {
      amount = parseBTH(amountStr)
      if (amount <= 0n) amountError = 'Amount must be greater than 0.'
    } catch {
      amountError = 'Enter a valid amount.'
    }
  }

  const canPay = addressValid && amount > 0n && !amountError && !isSending

  const handlePay = async () => {
    if (!canPay) return
    setError(null)
    setIsSending(true)
    try {
      const hash = await send(request.to, amount, request.memo)
      setTxHash(hash)
      await Promise.allSettled([refreshBalance(), refreshTransactions()])
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Payment failed.')
    } finally {
      setIsSending(false)
    }
  }

  if (txHash) {
    const explorerTxUrl = explorerBase ? `${explorerBase}/tx/${txHash}` : null
    return (
      <div className="space-y-4">
        <div className="flex flex-col items-center gap-2 text-center">
          <div className="w-12 h-12 rounded-full bg-success/10 flex items-center justify-center">
            <Check className="text-success" size={26} />
          </div>
          <p className="text-lg font-semibold text-light">Payment sent</p>
          <p className="text-sm text-ghost">
            Sent {formatBTH(amount)} BTH to {contactName ?? 'the requester'}.
          </p>
          {explorerTxUrl && (
            <a
              href={explorerTxUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-xs text-ghost hover:text-light"
            >
              View transaction <ExternalLink size={12} />
            </a>
          )}
        </div>
        <Link to="/wallet">
          <Button className="w-full justify-center">Go to my wallet</Button>
        </Link>
      </div>
    )
  }

  if (!addressValid) {
    return (
      <div className="flex items-start gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
        <AlertCircle size={16} className="shrink-0 mt-0.5" />
        <span>This payment-request link has an invalid recipient address.</span>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <div className="text-center">
        <p className="text-sm text-ghost">You&apos;re paying</p>
        <p className="font-display text-lg font-semibold text-light break-words">
          {contactName ?? 'a Botho address'}
        </p>
        <p className="text-xs text-ghost mt-1 font-mono break-all">{request.to}</p>
      </div>

      {isSelfPay && (
        <div className="flex items-start gap-2 p-3 rounded-lg bg-amber-500/10 border border-amber-500/20 text-amber-200/90 text-xs">
          <ShieldAlert size={15} className="text-amber-400 shrink-0 mt-0.5" />
          <span>This request is for your own address — you&apos;d be paying yourself.</span>
        </div>
      )}

      {request.memo && (
        <div>
          <label className="block text-sm text-ghost mb-1.5">Memo</label>
          <div className="px-3 py-2 rounded-lg bg-abyss border border-steel text-sm text-light break-words">
            {request.memo}
          </div>
        </div>
      )}

      <div>
        <div className="flex items-center justify-between mb-1.5">
          <label className="block text-sm text-ghost">Amount (BTH)</label>
          {balance && (
            <button
              type="button"
              onClick={() => setAmountStr(formatBTH(balance.available, { separators: false }))}
              className="text-xs text-pulse hover:underline"
            >
              Max: {formatBTH(balance.available)} BTH
            </button>
          )}
        </div>
        <Input
          type="text"
          inputMode="decimal"
          placeholder="0.00"
          value={amountStr}
          onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
            setAmountStr(e.target.value)
            setError(null)
          }}
          autoFocus={request.amount === undefined}
        />
        {request.amount === undefined && (
          <p className="text-xs text-ghost mt-1">
            The requester didn&apos;t specify an amount — enter how much to send.
          </p>
        )}
      </div>

      {amountError && (
        <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
          <AlertCircle size={16} className="shrink-0" />
          <span>{amountError}</span>
        </div>
      )}

      {error && (
        <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
          <AlertCircle size={16} className="shrink-0" />
          <span>{error}</span>
        </div>
      )}

      <Button onClick={handlePay} disabled={!canPay} className="w-full justify-center">
        {isSending ? (
          <>
            <Loader2 size={16} className="mr-2 animate-spin" />
            Sending…
          </>
        ) : (
          <>
            <Send size={16} className="mr-2" />
            Pay {amount > 0n ? `${formatBTH(amount)} BTH` : ''}
          </>
        )}
      </Button>
    </div>
  )
}

/**
 * Gate shown when there's no wallet, or the wallet is locked. Lets the visitor
 * unlock / create / import in-flow; the parsed request is preserved in the
 * parent's state, so the pay confirmation appears as soon as a wallet is ready.
 */
function WalletGate({
  isLocked,
  onCreate,
  onImport,
  onUnlock,
}: {
  isLocked: boolean
  onCreate: (mnemonic: string, password?: string) => Promise<void>
  onImport: (seedPhrase: string, password?: string) => Promise<void>
  onUnlock: (password: string) => Promise<void>
}) {
  const [mode, setMode] = useState<SetupMode>(isLocked ? 'unlock' : 'create')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // unlock
  const [password, setPassword] = useState('')
  // create
  const newMnemonic = useMemo(() => createMnemonic12(), [])
  const [revealed, setRevealed] = useState(false)
  const [confirmed, setConfirmed] = useState(false)
  // import
  const [seedPhrase, setSeedPhrase] = useState('')

  const handleUnlock = async () => {
    setBusy(true)
    setError(null)
    try {
      await onUnlock(password)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unlock failed.')
    } finally {
      setBusy(false)
    }
  }

  const handleCreate = async () => {
    setBusy(true)
    setError(null)
    try {
      await onCreate(newMnemonic)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not create wallet.')
    } finally {
      setBusy(false)
    }
  }

  const handleImport = async () => {
    setBusy(true)
    setError(null)
    try {
      await onImport(seedPhrase)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Import failed.')
    } finally {
      setBusy(false)
    }
  }

  const wordCount = seedPhrase.trim().split(/\s+/).filter(Boolean).length

  return (
    <div className="space-y-4">
      <p className="text-sm text-ghost text-center">
        {isLocked
          ? 'Unlock your wallet to pay this request.'
          : 'You need a wallet to pay this request.'}
      </p>

      {!isLocked && (
        <div className="flex rounded-lg bg-abyss border border-steel p-1">
          <button
            onClick={() => {
              setMode('create')
              setError(null)
            }}
            className={`flex-1 py-2 px-4 rounded-md text-sm font-medium transition-colors ${
              mode === 'create' ? 'bg-steel text-light' : 'text-ghost hover:text-light'
            }`}
          >
            Create New
          </button>
          <button
            onClick={() => {
              setMode('import')
              setError(null)
            }}
            className={`flex-1 py-2 px-4 rounded-md text-sm font-medium transition-colors ${
              mode === 'import' ? 'bg-steel text-light' : 'text-ghost hover:text-light'
            }`}
          >
            Import Existing
          </button>
        </div>
      )}

      {mode === 'unlock' && (
        <div className="space-y-3">
          <div className="flex justify-center">
            <Lock className="text-pulse" size={24} />
          </div>
          <Input
            type="password"
            placeholder="Enter password"
            value={password}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
              setPassword(e.target.value)
              setError(null)
            }}
            onKeyDown={(e: React.KeyboardEvent) => {
              if (e.key === 'Enter' && password) handleUnlock()
            }}
            autoFocus
          />
          <Button onClick={handleUnlock} disabled={!password || busy} className="w-full justify-center">
            {busy ? <Loader2 size={16} className="mr-2 animate-spin" /> : null}
            Unlock
          </Button>
        </div>
      )}

      {mode === 'create' && (
        <div className="space-y-3">
          <div className="relative">
            <div
              className={`p-3 rounded-lg bg-abyss border border-steel font-mono text-xs leading-relaxed ${
                revealed ? '' : 'blur-sm select-none'
              }`}
            >
              {newMnemonic}
            </div>
            {!revealed && (
              <button
                onClick={() => setRevealed(true)}
                className="absolute inset-0 flex items-center justify-center gap-2 text-ghost hover:text-light"
              >
                <Eye size={18} />
                <span className="text-sm">Click to reveal</span>
              </button>
            )}
          </div>
          <label className="flex items-start gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={confirmed}
              onChange={(e) => setConfirmed(e.target.checked)}
              className="mt-1 w-4 h-4 accent-pulse"
            />
            <span className="text-xs text-ghost">
              I&apos;ve written down my recovery phrase and stored it safely.
            </span>
          </label>
          <Button
            onClick={handleCreate}
            disabled={!revealed || !confirmed || busy}
            className="w-full justify-center"
          >
            {busy ? <Loader2 size={16} className="mr-2 animate-spin" /> : null}
            Create &amp; Continue
          </Button>
        </div>
      )}

      {mode === 'import' && (
        <div className="space-y-3">
          <textarea
            value={seedPhrase}
            onChange={(e) => {
              setSeedPhrase(e.target.value)
              setError(null)
            }}
            placeholder="Enter your 12 or 24 word recovery phrase…"
            rows={3}
            className="w-full p-3 rounded-lg bg-abyss border border-steel font-mono text-xs leading-relaxed resize-none focus:outline-none focus:ring-2 focus:ring-pulse/50 focus:border-pulse placeholder:text-ghost/50"
          />
          <Button
            onClick={handleImport}
            disabled={(wordCount !== 12 && wordCount !== 24) || busy}
            className="w-full justify-center"
          >
            {busy ? <Loader2 size={16} className="mr-2 animate-spin" /> : null}
            Import &amp; Continue
          </Button>
        </div>
      )}

      {error && (
        <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
          <AlertCircle size={16} className="shrink-0" />
          <span>{error}</span>
        </div>
      )}
    </div>
  )
}
