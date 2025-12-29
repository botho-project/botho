import { Layout } from '../components/layout'
import { Card, CardContent } from '@botho/ui'
import { Wallet } from 'lucide-react'

export function WalletPage() {
  return (
    <Layout title="Wallet" subtitle="Manage your BTH holdings">
      <Card>
        <CardContent className="flex flex-col items-center justify-center py-20">
          <Wallet className="h-16 w-16 text-[--color-dim]" />
          <p className="mt-4 text-lg text-[--color-ghost]">Wallet functionality coming soon</p>
          <p className="mt-2 text-sm text-[--color-dim]">
            The wallet page will be integrated with the local node
          </p>
        </CardContent>
      </Card>
    </Layout>
  )
}
