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
import { motion } from 'motion/react'
import { Lock, RefreshCw, Send, Wallet, Zap } from 'lucide-react'
import { useWallet } from '../contexts/wallet'
import { useConnection } from '../contexts/connection'

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

// Main wallet page
export function WalletPage() {
  const { connectedNode } = useConnection()
  const {
    address,
    balance,
    transactions,
    isLoading,
    isSending,
    error,
    refreshBalance,
    refreshTransactions,
    sendTransaction,
    estimateFee,
    setAddress,
  } = useWallet()

  const [showSendModal, setShowSendModal] = useState(false)

  // Wrap sendTransaction to match SendModal interface
  const handleSend = useCallback(
    async (data: SendFormData): Promise<SendResult> => {
      return sendTransaction({
        recipient: data.recipient,
        amount: data.amount,
        privacyLevel: data.privacyLevel,
        memo: data.memo,
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
              <Button onClick={() => setShowSendModal(true)}>
                <Send className="h-4 w-4" />
                Send
              </Button>
            </>
          }
        />

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
