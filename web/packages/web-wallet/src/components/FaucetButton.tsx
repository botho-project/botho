import { useState } from 'react'
import { Button, Card } from '@botho/ui'
import { Droplets, Loader2, Check, AlertCircle, ExternalLink } from 'lucide-react'
import { useNetwork } from '../contexts/network'
import { useWallet } from '../contexts/wallet'

interface FaucetResponse {
  success: boolean
  txHash?: string
  amount?: number
  error?: string
  retryAfter?: number
}

export function FaucetButton() {
  const { network, hasFaucet } = useNetwork()
  const { address } = useWallet()
  const [isRequesting, setIsRequesting] = useState(false)
  const [result, setResult] = useState<FaucetResponse | null>(null)

  if (!hasFaucet || !network.faucetEndpoint) {
    return null
  }

  const handleRequest = async () => {
    if (!address) return

    setIsRequesting(true)
    setResult(null)

    try {
      const response = await fetch(network.faucetEndpoint!, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          method: 'faucet_request',
          params: { address },
          id: 1,
        }),
      })

      const json = await response.json()

      if (json.error) {
        // Check if rate limited
        if (json.error.code === -32001 || json.error.message?.includes('rate')) {
          const retryMatch = json.error.message?.match(/(\d+)\s*(seconds?|minutes?|hours?)/)
          let retryAfter: number | undefined

          if (retryMatch) {
            const value = parseInt(retryMatch[1], 10)
            const unit = retryMatch[2].toLowerCase()
            if (unit.startsWith('minute')) {
              retryAfter = value * 60
            } else if (unit.startsWith('hour')) {
              retryAfter = value * 3600
            } else {
              retryAfter = value
            }
          }

          setResult({
            success: false,
            error: 'Rate limited. Please try again later.',
            retryAfter,
          })
        } else {
          setResult({
            success: false,
            error: json.error.message || 'Faucet request failed',
          })
        }
      } else if (json.result) {
        setResult({
          success: true,
          txHash: json.result.txHash,
          amount: json.result.amount,
        })
      }
    } catch (err) {
      setResult({
        success: false,
        error: err instanceof Error ? err.message : 'Request failed',
      })
    } finally {
      setIsRequesting(false)
    }
  }

  const formatRetryTime = (seconds: number): string => {
    if (seconds >= 3600) {
      const hours = Math.floor(seconds / 3600)
      return `${hours} hour${hours > 1 ? 's' : ''}`
    }
    if (seconds >= 60) {
      const minutes = Math.floor(seconds / 60)
      return `${minutes} minute${minutes > 1 ? 's' : ''}`
    }
    return `${seconds} second${seconds > 1 ? 's' : ''}`
  }

  const explorerTxUrl = result?.txHash && network.explorerUrl
    ? `${network.explorerUrl}/tx/${result.txHash}`
    : null

  return (
    <Card className="p-4 sm:p-5">
      <div className="flex items-start gap-4">
        <div className="w-10 h-10 rounded-full bg-pulse/10 flex items-center justify-center shrink-0">
          <Droplets className="text-pulse" size={20} />
        </div>
        <div className="flex-1 min-w-0">
          <h3 className="font-medium text-light mb-1">Get Testnet BTH</h3>
          <p className="text-sm text-ghost mb-4">
            Request free testnet coins for testing. Limited to one request per hour.
          </p>

          {result && (
            <div className={`mb-4 p-3 rounded-lg ${
              result.success
                ? 'bg-success/10 border border-success/20'
                : 'bg-danger/10 border border-danger/20'
            }`}>
              <div className="flex items-start gap-2">
                {result.success ? (
                  <Check size={16} className="text-success mt-0.5 shrink-0" />
                ) : (
                  <AlertCircle size={16} className="text-danger mt-0.5 shrink-0" />
                )}
                <div className="flex-1 min-w-0">
                  {result.success ? (
                    <>
                      <p className="text-sm text-success">
                        Received {result.amount ? `${result.amount / 1e12} BTH` : 'testnet BTH'}
                      </p>
                      {explorerTxUrl && (
                        <a
                          href={explorerTxUrl}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="inline-flex items-center gap-1 text-xs text-ghost hover:text-light mt-1"
                        >
                          View transaction
                          <ExternalLink size={12} />
                        </a>
                      )}
                    </>
                  ) : (
                    <>
                      <p className="text-sm text-danger">{result.error}</p>
                      {result.retryAfter && (
                        <p className="text-xs text-ghost mt-1">
                          Try again in {formatRetryTime(result.retryAfter)}
                        </p>
                      )}
                    </>
                  )}
                </div>
              </div>
            </div>
          )}

          <Button
            onClick={handleRequest}
            disabled={!address || isRequesting}
            size="sm"
          >
            {isRequesting ? (
              <>
                <Loader2 size={14} className="mr-2 animate-spin" />
                Requesting...
              </>
            ) : (
              <>
                <Droplets size={14} className="mr-2" />
                Request BTH
              </>
            )}
          </Button>
        </div>
      </div>
    </Card>
  )
}
