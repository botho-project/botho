import { useCallback, useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, Input, Logo } from '@botho/ui'
import {
  parseClaimLinkFragment,
  isValidAddress,
  formatBTH,
  createMnemonic12,
  saveWallet,
  deriveAddress,
  type ClaimLinkSecret,
} from '@botho/core'
import {
  Gift,
  AlertCircle,
  Check,
  Loader2,
  ArrowLeft,
  ShieldAlert,
  Clock,
  Eye,
  ExternalLink,
} from 'lucide-react'
import { useAdapter } from '../contexts/wallet'
import { useNetwork } from '../contexts/network'
import { PasswordFields, isPasswordValid } from '../components/PasswordSettingsModal'
import { scanEphemeral, sweepEphemeral, type EphemeralScan } from '../lib/claim-link-ops'

/**
 * P2 — Claim page for claimable payment links (#460).
 *
 * Reads the ephemeral secret from the URL FRAGMENT (never sent to a server),
 * strips it from the address bar, reconstructs the ephemeral wallet, scans for
 * its spendable output(s), and lets the recipient sweep the funds to any
 * address — pasting an existing one or creating a brand-new wallet in-flow.
 *
 * Chain is the source of truth: a swept (already-claimed) or not-yet-confirmed
 * link both scan to an empty spendable set; we distinguish via state.
 *
 * UNFURL-SAFETY INVARIANT (#589): parsing the fragment is a pure, local,
 * non-network operation. The page performs NO network fetch keyed on the bearer
 * secret until the recipient EXPLICITLY acts (clicks "Reveal"). So even if a
 * link-preview / unfurl bot were to load this page WITH the fragment (it
 * normally can't — browsers never send the fragment to a server), it would
 * never trigger an on-chain scan or claim, nor leak the secret. The `claim-page
 * performs no fetch before user action` regression test in `claim.test.tsx`
 * locks this in.
 */

type ClaimState =
  | 'parsing'
  | 'ready' // secret parsed & held locally; awaiting explicit user action (no network yet)
  | 'invalid'
  | 'scanning'
  | 'waiting' // funding not yet confirmed
  | 'claimable'
  | 'already-claimed'
  | 'sweeping'
  | 'claimed'

export function ClaimPage() {
  const adapter = useAdapter()
  const { network } = useNetwork()

  // Capture the URL fragment SYNCHRONOUSLY, exactly once, at state-init time —
  // BEFORE any effect runs. The mount effect below strips the fragment (#589),
  // so a second effect invocation (React StrictMode double-invokes effects in
  // dev) would otherwise read an empty hash and clobber the parsed 'ready' state
  // with the "not found" error. Reading it here makes the effect idempotent.
  const [initialHash] = useState<string>(() => window.location.hash)

  const [state, setState] = useState<ClaimState>('parsing')
  const [secret, setSecret] = useState<ClaimLinkSecret | null>(null)
  const [scan, setScan] = useState<EphemeralScan | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [destination, setDestination] = useState('')
  const [claimTxHash, setClaimTxHash] = useState<string | null>(null)
  const [createdMnemonic, setCreatedMnemonic] = useState<string | null>(null)
  const [showNewWallet, setShowNewWallet] = useState(false)
  // SECURITY (#672): an in-flow-created wallet follows the same #475 policy as
  // the main setup — a password is REQUIRED so the persisted seed is encrypted
  // at rest, not written to localStorage in plaintext.
  const [newWalletPassword, setNewWalletPassword] = useState('')
  const [confirmNewWalletPassword, setConfirmNewWalletPassword] = useState('')
  const newWalletPasswordValid = isPasswordValid(newWalletPassword, confirmNewWalletPassword)
  const persistingNewWallet = showNewWallet && createdMnemonic !== null

  // Track whether we've already begun a sweep so a late re-scan can't downgrade
  // the state out from under the user.
  const sweepingRef = useRef(false)

  // 1. Parse the fragment ONCE on mount, then strip it from the URL so the
  //    bearer secret does not linger in the address bar / history.
  //
  //    NOTE (#589): parsing is purely local — NO network call happens here. We
  //    land in the 'ready' state and wait for the recipient to explicitly act
  //    before touching the node (see `handleReveal`). This is the unfurl-safety
  //    invariant: a preview/unfurl load can never trigger a scan or claim.
  useEffect(() => {
    if (!initialHash || initialHash === '#') {
      setState('invalid')
      setError('No claim link found. The link should look like .../claim#…')
      return
    }
    try {
      const parsed = parseClaimLinkFragment(initialHash)
      setSecret(parsed)
      // Strip the fragment so the secret is not visible/logged after reading.
      try {
        window.history.replaceState(null, '', window.location.pathname + window.location.search)
      } catch {
        // replaceState may be unavailable in some embeds; non-fatal.
      }
      // Do NOT scan yet — wait for an explicit user action (unfurl-safety).
      setState('ready')
    } catch (err) {
      setState('invalid')
      setError(err instanceof Error ? err.message : 'This claim link is not valid.')
    }
  }, [initialHash])

  // Explicit user action that begins the first network call (the scan). Keeping
  // the scan behind this gate is what makes a preview/unfurl fetch a no-op.
  const handleReveal = useCallback(() => {
    setError(null)
    setState('scanning')
  }, [])

  const runScan = useCallback(async () => {
    if (!secret) return
    if (sweepingRef.current) return
    try {
      const result = await scanEphemeral(adapter, secret.mnemonic)
      if (sweepingRef.current) return
      setScan(result)
      if (result.gross > 0n) {
        setState('claimable')
      } else {
        // Empty spendable set: either not yet confirmed, or already claimed.
        // We keep "waiting" until the user has waited a bit; default to waiting
        // so a fresh link that's mid-confirmation shows a friendly message.
        setState((prev) => (prev === 'claimable' ? 'already-claimed' : 'waiting'))
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to scan the link.')
      setState('invalid')
    }
  }, [adapter, secret])

  // 2. Scan once the secret is parsed, and re-scan on each new block while we
  //    are still waiting for the funding to confirm.
  useEffect(() => {
    if (state === 'scanning' || state === 'waiting') {
      runScan()
    }
  }, [state, runScan])

  useEffect(() => {
    if (state !== 'waiting') return
    const unsub = adapter.onNewBlock(() => {
      runScan()
    })
    return unsub
  }, [state, adapter, runScan])

  const handleCreateWallet = () => {
    const m = createMnemonic12()
    setCreatedMnemonic(m)
    setDestination(deriveAddress(m))
    setShowNewWallet(true)
  }

  const handleSweep = async () => {
    if (!secret) return
    const dest = destination.trim()
    if (!isValidAddress(dest)) {
      setError('Enter a valid destination address (tbotho://… or botho://…).')
      return
    }
    if (persistingNewWallet && !newWalletPasswordValid) {
      setError('Set a password for your new wallet before claiming.')
      return
    }
    setError(null)
    sweepingRef.current = true
    setState('sweeping')
    try {
      // If the recipient created a new wallet in-flow, persist it so they can
      // use it afterwards in this browser — encrypted under their password
      // (#672/#475; saveWallet with a password writes a vault blob).
      if (createdMnemonic && showNewWallet) {
        await saveWallet(createdMnemonic, newWalletPassword)
      }
      const { txHash } = await sweepEphemeral(adapter, secret.mnemonic, dest)
      setClaimTxHash(txHash)
      setState('claimed')
    } catch (err) {
      sweepingRef.current = false
      const msg = err instanceof Error ? err.message : 'Claim failed.'
      // A double-spend / empty-set error means someone else claimed first.
      if (/already claimed|empty|double|nothing to claim|spent|insufficient/i.test(msg)) {
        setState('already-claimed')
      } else {
        setError(msg)
        setState('claimable')
      }
    }
  }

  const explorerTxUrl =
    claimTxHash && network.explorerUrl ? `${network.explorerUrl}/tx/${claimTxHash}` : null

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">Botho Wallet</span>
          </Link>
        </div>
      </header>

      <main className="py-8 sm:py-12">
        <div className="max-w-lg mx-auto px-4 sm:px-0">
          <Card className="p-5 sm:p-8">
            <div className="text-center mb-6">
              <div className="w-14 h-14 sm:w-16 sm:h-16 rounded-full bg-pulse/10 flex items-center justify-center mx-auto mb-3 sm:mb-4">
                <Gift className="text-pulse" size={28} />
              </div>
              <h2 className="font-display text-xl sm:text-2xl font-bold mb-2">Claim Your BTH</h2>
            </div>

            {(state === 'parsing' || state === 'scanning') && (
              <div className="flex flex-col items-center gap-3 py-6 text-ghost">
                <Loader2 size={28} className="animate-spin text-pulse" />
                <p className="text-sm">
                  {state === 'scanning' ? 'Checking your claim link…' : 'Reading your claim link…'}
                </p>
              </div>
            )}

            {state === 'ready' && (
              <div className="space-y-4">
                <div className="text-center">
                  <p className="text-sm text-ghost">Someone sent you a claim link.</p>
                  {secret?.amountHint !== undefined && (
                    <p className="font-display text-2xl font-bold text-pulse mt-1">
                      ~{formatBTH(secret.amountHint)} BTH
                    </p>
                  )}
                </div>
                <Button onClick={handleReveal} className="w-full justify-center">
                  <Gift size={16} className="mr-2" />
                  Reveal &amp; check this link
                </Button>
                <p className="text-xs text-ghost text-center">
                  We only contact the network once you tap above — your link stays private until
                  then.
                </p>
              </div>
            )}

            {state === 'invalid' && (
              <div className="flex items-start gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
                <AlertCircle size={16} className="shrink-0 mt-0.5" />
                <span>{error ?? 'This claim link is not valid.'}</span>
              </div>
            )}

            {state === 'waiting' && (
              <div className="flex flex-col items-center gap-3 py-4 text-center">
                <Clock size={28} className="text-amber-400" />
                <p className="text-sm text-ghost">
                  Waiting for the sender&apos;s payment to confirm on-chain. This page will
                  update automatically — it can take a minute or two.
                </p>
                <Loader2 size={18} className="animate-spin text-pulse" />
              </div>
            )}

            {state === 'already-claimed' && (
              <div className="flex items-start gap-2 p-3 rounded-lg bg-steel/40 border border-steel text-light text-sm">
                <Check size={16} className="shrink-0 mt-0.5 text-ghost" />
                <span>This link has already been claimed.</span>
              </div>
            )}

            {(state === 'claimable' || state === 'sweeping') && scan && (
              <div className="space-y-4">
                <div className="text-center">
                  <p className="text-sm text-ghost">You&apos;ve been sent</p>
                  <p className="font-display text-3xl font-bold text-pulse">{formatBTH(scan.net)} BTH</p>
                  <p className="text-xs text-ghost mt-1">
                    Network fee of {formatBTH(scan.fee)} BTH covered by the sender.
                  </p>
                </div>

                <div>
                  <label className="block text-sm text-ghost mb-1.5">Send to address</label>
                  <Input
                    type="text"
                    placeholder="tbotho://1/…"
                    value={destination}
                    onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
                      setDestination(e.target.value)
                      setError(null)
                      setShowNewWallet(false)
                    }}
                    disabled={state === 'sweeping'}
                  />
                  <button
                    type="button"
                    onClick={handleCreateWallet}
                    disabled={state === 'sweeping'}
                    className="text-xs text-pulse hover:underline mt-2"
                  >
                    Don&apos;t have a wallet? Create one
                  </button>
                </div>

                {showNewWallet && createdMnemonic && (
                  <div className="p-3 rounded-lg bg-amber-500/10 border border-amber-500/20 space-y-2">
                    <div className="flex items-center gap-2 text-amber-300 text-sm font-medium">
                      <Eye size={15} /> Your new recovery phrase
                    </div>
                    <p className="font-mono text-xs leading-relaxed text-amber-100/90 break-words">
                      {createdMnemonic}
                    </p>
                    <p className="text-xs text-amber-200/80">
                      Write this down and keep it safe — it is the ONLY way to recover this
                      wallet. We&apos;ll save it encrypted in this browser when you claim.
                    </p>
                    <div className="pt-1">
                      <p className="text-xs text-amber-200/80 mb-2">
                        Set a password — your wallet is encrypted on this device with it.
                      </p>
                      <PasswordFields
                        password={newWalletPassword}
                        confirmPassword={confirmNewWalletPassword}
                        onPassword={setNewWalletPassword}
                        onConfirmPassword={setConfirmNewWalletPassword}
                      />
                    </div>
                  </div>
                )}

                {error && (
                  <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
                    <AlertCircle size={16} className="shrink-0" />
                    <span>{error}</span>
                  </div>
                )}

                <Button
                  onClick={handleSweep}
                  disabled={
                    state === 'sweeping' ||
                    !destination.trim() ||
                    (persistingNewWallet && !newWalletPasswordValid)
                  }
                  className="w-full justify-center"
                >
                  {state === 'sweeping' ? (
                    <><Loader2 size={16} className="mr-2 animate-spin" />Claiming…</>
                  ) : (
                    <>Claim {formatBTH(scan.net)} BTH</>
                  )}
                </Button>

                <div className="flex items-start gap-2 text-xs text-ghost">
                  <ShieldAlert size={14} className="shrink-0 mt-0.5" />
                  <span>
                    This is a bearer link — anyone with it can claim. Claim promptly so no
                    one else does first.
                  </span>
                </div>
              </div>
            )}

            {state === 'claimed' && (
              <div className="space-y-4">
                <div className="flex flex-col items-center gap-2 text-center">
                  <div className="w-12 h-12 rounded-full bg-success/10 flex items-center justify-center">
                    <Check className="text-success" size={26} />
                  </div>
                  <p className="text-lg font-semibold text-light">
                    Claimed {scan ? formatBTH(scan.net) : ''} BTH
                  </p>
                  <p className="text-sm text-ghost">The funds are on their way to your address.</p>
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

                {/* Post-claim hygiene (#589): the link is now spent, but its
                    bearer secret still lingers in the chat. Nudge the recipient
                    to delete the message to cut its dwell time in history. */}
                <div className="flex items-start gap-2 p-3 rounded-lg bg-steel/40 border border-steel text-xs text-ghost">
                  <ShieldAlert size={14} className="shrink-0 mt-0.5 text-amber-400" />
                  <span>
                    For your privacy, delete the message that contained this link from your chat.
                    It&apos;s already claimed and can&apos;t be used again, but removing it keeps the
                    secret out of your message history and backups.
                  </span>
                </div>

                <Link to="/wallet">
                  <Button className="w-full justify-center">Go to my wallet</Button>
                </Link>
              </div>
            )}
          </Card>
        </div>
      </main>
    </div>
  )
}
