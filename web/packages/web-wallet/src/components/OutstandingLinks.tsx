import { useEffect, useState } from 'react'
import { Button, Card } from '@botho/ui'
import { formatBTH, shortenAddress, buildClaimLink } from '@botho/core'
import { Link2, Copy, Check, RotateCcw, Trash2, RefreshCw } from 'lucide-react'
import { useWallet } from '../contexts/wallet'

/**
 * P3 — Outstanding claim links (sender side, #460).
 *
 * Lists locally-tracked claim links with their status, lets the sender copy a
 * link again, reclaim (refund) an unclaimed link's funds, or forget a record.
 * Status is refreshed by re-scanning each ephemeral wallet (chain is the source
 * of truth: a swept output reads back as "claimed").
 */
export function OutstandingLinks() {
  const { claimLinks, refreshClaimLinks, refundClaimLink, forgetClaimLink } = useWallet()
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [copiedId, setCopiedId] = useState<string | null>(null)
  const [refreshing, setRefreshing] = useState(false)

  useEffect(() => {
    // Refresh statuses once when the list is shown.
    setRefreshing(true)
    refreshClaimLinks().finally(() => setRefreshing(false))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  if (claimLinks.length === 0) return null

  const handleRefresh = async () => {
    setRefreshing(true)
    setError(null)
    try {
      await refreshClaimLinks()
    } finally {
      setRefreshing(false)
    }
  }

  const handleCopy = async (id: string, ephMnemonic: string, amount: bigint) => {
    const origin =
      typeof window !== 'undefined' && window.location?.origin
        ? window.location.origin
        : 'https://wallet.botho.io'
    const url = buildClaimLink(origin, ephMnemonic, amount)
    try {
      await navigator.clipboard.writeText(url)
      setCopiedId(id)
      setTimeout(() => setCopiedId(null), 2000)
    } catch {
      // ignore clipboard failures
    }
  }

  const handleRefund = async (id: string) => {
    setBusyId(id)
    setError(null)
    try {
      await refundClaimLink(id)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Refund failed')
    } finally {
      setBusyId(null)
    }
  }

  const statusLabel = (status: string) => {
    switch (status) {
      case 'claimed':
        return <span className="text-xs text-ghost">Claimed</span>
      case 'refunded':
        return <span className="text-xs text-ghost">Refunded</span>
      default:
        return <span className="text-xs text-amber-400">Outstanding</span>
    }
  }

  return (
    <Card className="p-4 sm:p-5">
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-2">
          <Link2 className="text-pulse" size={18} />
          <h3 className="font-medium text-light">Outstanding Links</h3>
        </div>
        <Button variant="ghost" size="sm" onClick={handleRefresh} disabled={refreshing} title="Refresh statuses">
          <RefreshCw size={16} className={refreshing ? 'animate-spin' : ''} />
        </Button>
      </div>

      {error && (
        <div className="mb-3 p-2.5 rounded-lg bg-danger/10 border border-danger/20 text-danger text-xs">
          {error}
        </div>
      )}

      <div className="space-y-2">
        {claimLinks.map((link) => (
          <div
            key={link.id}
            className="flex items-center justify-between gap-3 p-3 rounded-lg bg-abyss border border-steel"
          >
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <span className="text-sm text-light font-medium">{formatBTH(link.amount)} BTH</span>
                {statusLabel(link.status)}
              </div>
              <p className="text-xs text-ghost font-mono truncate">{shortenAddress(link.ephAddress)}</p>
            </div>
            <div className="flex items-center gap-1 shrink-0">
              <Button
                variant="ghost"
                size="sm"
                title="Copy link"
                onClick={() => handleCopy(link.id, link.ephMnemonic, link.amount)}
              >
                {copiedId === link.id ? <Check size={15} /> : <Copy size={15} />}
              </Button>
              {link.status === 'outstanding' && (
                <Button
                  variant="ghost"
                  size="sm"
                  title="Reclaim funds"
                  disabled={busyId === link.id}
                  onClick={() => handleRefund(link.id)}
                >
                  {busyId === link.id ? (
                    <RefreshCw size={15} className="animate-spin" />
                  ) : (
                    <RotateCcw size={15} />
                  )}
                </Button>
              )}
              <Button
                variant="ghost"
                size="sm"
                title="Forget"
                onClick={() => forgetClaimLink(link.id)}
              >
                <Trash2 size={15} />
              </Button>
            </div>
          </div>
        ))}
      </div>
    </Card>
  )
}
