import { Layout } from '@/components/layout'
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card'
import { motion } from 'motion/react'
import {
  Wallet,
  Send,
  Download,
  Copy,
  Eye,
  EyeOff,
  ArrowUpRight,
  ArrowDownLeft,
  Clock,
  CheckCircle2,
  XCircle,
  Shield,
  ShieldOff,
} from 'lucide-react'
import { useState } from 'react'
import { cn, formatNumber, timeAgo } from '@/lib/utils'

interface Transaction {
  id: string
  type: 'send' | 'receive' | 'mining'
  amount: number
  fee: number
  txType: 'plain' | 'hidden' | 'mining'
  status: 'confirmed' | 'pending' | 'failed'
  timestamp: number
  counterparty?: string
  confirmations: number
}

const mockTransactions: Transaction[] = [
  { id: 'tx1a2b3c4d', type: 'receive', amount: 125.5, fee: 0, txType: 'hidden', status: 'confirmed', timestamp: Date.now() / 1000 - 300, counterparty: '0x1234...5678', confirmations: 12 },
  { id: 'tx2b3c4d5e', type: 'send', amount: 50.0, fee: 0.025, txType: 'plain', status: 'confirmed', timestamp: Date.now() / 1000 - 1800, counterparty: '0xabcd...ef01', confirmations: 45 },
  { id: 'tx3c4d5e6f', type: 'mining', amount: 2.5, fee: 0, txType: 'mining', status: 'confirmed', timestamp: Date.now() / 1000 - 3600, confirmations: 89 },
  { id: 'tx4d5e6f7g', type: 'send', amount: 200.0, fee: 1.2, txType: 'hidden', status: 'pending', timestamp: Date.now() / 1000 - 60, counterparty: '0x9876...5432', confirmations: 0 },
  { id: 'tx5e6f7g8h', type: 'receive', amount: 75.25, fee: 0, txType: 'plain', status: 'confirmed', timestamp: Date.now() / 1000 - 7200, counterparty: '0xfedc...ba98', confirmations: 120 },
]

const txTypeColors = {
  plain: { bg: 'bg-[--color-pulse]/10', text: 'text-[--color-pulse]', label: 'Plain' },
  hidden: { bg: 'bg-[--color-purple]/10', text: 'text-[--color-purple]', label: 'Hidden' },
  mining: { bg: 'bg-[--color-success]/10', text: 'text-[--color-success]', label: 'Mining' },
}

const statusIcons = {
  confirmed: CheckCircle2,
  pending: Clock,
  failed: XCircle,
}

const statusColors = {
  confirmed: 'text-[--color-success]',
  pending: 'text-[--color-warning]',
  failed: 'text-[--color-danger]',
}

export function WalletPage() {
  const [showBalance, setShowBalance] = useState(true)
  const [copied, setCopied] = useState(false)

  const balance = 1234.56789
  const pendingBalance = 200.0
  const address = 'cad1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh'

  const copyAddress = () => {
    navigator.clipboard.writeText(address)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  return (
    <Layout title="Wallet" subtitle="Manage your CAD holdings">
      <div className="grid grid-cols-3 gap-6">
        {/* Main wallet card */}
        <div className="col-span-2 space-y-6">
          {/* Balance card */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            className="relative overflow-hidden rounded-2xl border border-[--color-steel] bg-gradient-to-br from-[--color-abyss] to-[--color-slate] p-6"
          >
            {/* Background decoration */}
            <div className="absolute -right-20 -top-20 h-64 w-64 rounded-full bg-[--color-pulse]/5 blur-3xl" />
            <div className="absolute -bottom-20 -left-20 h-64 w-64 rounded-full bg-[--color-purple]/5 blur-3xl" />

            <div className="relative">
              <div className="flex items-start justify-between">
                <div>
                  <p className="font-display text-sm font-semibold uppercase tracking-wider text-[--color-dim]">
                    Total Balance
                  </p>
                  <div className="mt-2 flex items-baseline gap-3">
                    <motion.p
                      key={showBalance ? 'show' : 'hide'}
                      initial={{ opacity: 0, y: 10 }}
                      animate={{ opacity: 1, y: 0 }}
                      className="font-display text-5xl font-bold text-[--color-light]"
                    >
                      {showBalance ? formatNumber(balance) : '••••••'}
                    </motion.p>
                    <span className="text-xl text-[--color-dim]">CAD</span>
                  </div>
                  {pendingBalance > 0 && (
                    <p className="mt-2 text-sm text-[--color-warning]">
                      +{formatNumber(pendingBalance)} CAD pending
                    </p>
                  )}
                </div>
                <button
                  onClick={() => setShowBalance(!showBalance)}
                  className="rounded-lg p-2 text-[--color-ghost] transition-colors hover:bg-[--color-steel] hover:text-[--color-light]"
                >
                  {showBalance ? <EyeOff className="h-5 w-5" /> : <Eye className="h-5 w-5" />}
                </button>
              </div>

              {/* Address */}
              <div className="mt-6">
                <p className="text-xs text-[--color-dim]">Wallet Address</p>
                <div className="mt-1 flex items-center gap-2">
                  <code className="font-mono text-sm text-[--color-soft]">{address}</code>
                  <button
                    onClick={copyAddress}
                    className="rounded p-1 text-[--color-ghost] transition-colors hover:bg-[--color-steel] hover:text-[--color-pulse]"
                  >
                    <Copy className="h-4 w-4" />
                  </button>
                  {copied && (
                    <span className="text-xs text-[--color-success]">Copied!</span>
                  )}
                </div>
              </div>

              {/* Action buttons */}
              <div className="mt-6 flex gap-3">
                <button className="flex flex-1 items-center justify-center gap-2 rounded-xl bg-[--color-pulse] px-4 py-3 font-display font-semibold text-[--color-void] transition-all hover:bg-[--color-pulse-dim] hover:shadow-[0_0_20px_rgba(6,182,212,0.3)]">
                  <Send className="h-5 w-5" />
                  Send
                </button>
                <button className="flex flex-1 items-center justify-center gap-2 rounded-xl border border-[--color-steel] bg-[--color-slate] px-4 py-3 font-display font-semibold text-[--color-light] transition-all hover:border-[--color-pulse-dim] hover:bg-[--color-steel]">
                  <Download className="h-5 w-5" />
                  Receive
                </button>
              </div>
            </div>
          </motion.div>

          {/* Transaction history */}
          <Card>
            <CardHeader>
              <div className="flex items-center gap-2">
                <Clock className="h-4 w-4 text-[--color-pulse]" />
                <CardTitle>Transaction History</CardTitle>
              </div>
              <div className="flex gap-2">
                <button className="rounded-lg bg-[--color-pulse]/10 px-3 py-1 text-xs font-medium text-[--color-pulse]">
                  All
                </button>
                <button className="rounded-lg px-3 py-1 text-xs font-medium text-[--color-dim] hover:bg-[--color-steel]">
                  Sent
                </button>
                <button className="rounded-lg px-3 py-1 text-xs font-medium text-[--color-dim] hover:bg-[--color-steel]">
                  Received
                </button>
              </div>
            </CardHeader>
            <CardContent className="p-0">
              <div className="divide-y divide-[--color-steel]">
                {mockTransactions.map((tx, i) => {
                  const StatusIcon = statusIcons[tx.status]
                  const typeColor = txTypeColors[tx.txType]

                  return (
                    <motion.div
                      key={tx.id}
                      initial={{ opacity: 0, x: -20 }}
                      animate={{ opacity: 1, x: 0 }}
                      transition={{ delay: i * 0.05 }}
                      className="group flex items-center justify-between px-5 py-4 transition-colors hover:bg-[--color-slate]/50"
                    >
                      <div className="flex items-center gap-4">
                        <div className={cn(
                          'flex h-10 w-10 items-center justify-center rounded-full',
                          tx.type === 'receive' || tx.type === 'mining'
                            ? 'bg-[--color-success]/10'
                            : 'bg-[--color-danger]/10'
                        )}>
                          {tx.type === 'receive' ? (
                            <ArrowDownLeft className="h-5 w-5 text-[--color-success]" />
                          ) : tx.type === 'mining' ? (
                            <Wallet className="h-5 w-5 text-[--color-success]" />
                          ) : (
                            <ArrowUpRight className="h-5 w-5 text-[--color-danger]" />
                          )}
                        </div>
                        <div>
                          <div className="flex items-center gap-2">
                            <span className="font-medium text-[--color-light] capitalize">
                              {tx.type === 'mining' ? 'Mining Reward' : tx.type}
                            </span>
                            <span className={cn('rounded px-1.5 py-0.5 text-xs font-medium', typeColor.bg, typeColor.text)}>
                              {tx.txType === 'hidden' ? <Shield className="inline h-3 w-3 mr-0.5" /> : tx.txType === 'plain' ? <ShieldOff className="inline h-3 w-3 mr-0.5" /> : null}
                              {typeColor.label}
                            </span>
                          </div>
                          <div className="flex items-center gap-2 text-xs text-[--color-dim]">
                            <span>{timeAgo(tx.timestamp)}</span>
                            {tx.counterparty && (
                              <>
                                <span>•</span>
                                <span className="font-mono">{tx.counterparty}</span>
                              </>
                            )}
                          </div>
                        </div>
                      </div>
                      <div className="text-right">
                        <p className={cn(
                          'font-mono text-lg font-medium',
                          tx.type === 'receive' || tx.type === 'mining'
                            ? 'text-[--color-success]'
                            : 'text-[--color-light]'
                        )}>
                          {tx.type === 'receive' || tx.type === 'mining' ? '+' : '-'}
                          {tx.amount.toFixed(2)} CAD
                        </p>
                        <div className="flex items-center justify-end gap-1 text-xs">
                          <StatusIcon className={cn('h-3 w-3', statusColors[tx.status])} />
                          <span className={statusColors[tx.status]}>
                            {tx.status === 'confirmed' ? `${tx.confirmations} conf` : tx.status}
                          </span>
                        </div>
                      </div>
                    </motion.div>
                  )
                })}
              </div>
            </CardContent>
          </Card>
        </div>

        {/* Sidebar */}
        <div className="space-y-6">
          {/* Quick stats */}
          <Card>
            <CardHeader>
              <CardTitle>Wallet Stats</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex justify-between">
                <span className="text-sm text-[--color-dim]">Total Received</span>
                <span className="font-mono text-sm text-[--color-success]">+2,450.00 CAD</span>
              </div>
              <div className="flex justify-between">
                <span className="text-sm text-[--color-dim]">Total Sent</span>
                <span className="font-mono text-sm text-[--color-light]">-1,215.43 CAD</span>
              </div>
              <div className="flex justify-between">
                <span className="text-sm text-[--color-dim]">Mining Rewards</span>
                <span className="font-mono text-sm text-[--color-success]">+125.00 CAD</span>
              </div>
              <div className="flex justify-between">
                <span className="text-sm text-[--color-dim]">Fees Paid</span>
                <span className="font-mono text-sm text-[--color-warning]">-5.67 CAD</span>
              </div>
              <div className="border-t border-[--color-steel] pt-4">
                <div className="flex justify-between">
                  <span className="text-sm font-medium text-[--color-soft]">Transactions</span>
                  <span className="font-mono text-sm text-[--color-light]">47</span>
                </div>
              </div>
            </CardContent>
          </Card>

          {/* Privacy breakdown */}
          <Card>
            <CardHeader>
              <CardTitle>Privacy Usage</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="space-y-3">
                <div>
                  <div className="mb-1 flex justify-between text-sm">
                    <span className="flex items-center gap-1.5 text-[--color-dim]">
                      <Shield className="h-3 w-3 text-[--color-purple]" />
                      Hidden Transactions
                    </span>
                    <span className="text-[--color-light]">68%</span>
                  </div>
                  <div className="h-2 overflow-hidden rounded-full bg-[--color-slate]">
                    <div className="h-full rounded-full bg-[--color-purple]" style={{ width: '68%' }} />
                  </div>
                </div>
                <div>
                  <div className="mb-1 flex justify-between text-sm">
                    <span className="flex items-center gap-1.5 text-[--color-dim]">
                      <ShieldOff className="h-3 w-3 text-[--color-pulse]" />
                      Plain Transactions
                    </span>
                    <span className="text-[--color-light]">32%</span>
                  </div>
                  <div className="h-2 overflow-hidden rounded-full bg-[--color-slate]">
                    <div className="h-full rounded-full bg-[--color-pulse]" style={{ width: '32%' }} />
                  </div>
                </div>
              </div>
              <p className="mt-4 text-xs text-[--color-dim]">
                You've saved ~45.20 CAD in fees by using plain transactions when privacy wasn't needed.
              </p>
            </CardContent>
          </Card>
        </div>
      </div>
    </Layout>
  )
}
