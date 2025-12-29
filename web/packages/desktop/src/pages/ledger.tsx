import { Layout } from '../components/layout'
import { Card, CardContent } from '@botho/ui'
import { Database } from 'lucide-react'

export function LedgerPage() {
  return (
    <Layout title="Ledger" subtitle="Browse blocks and transactions">
      <Card>
        <CardContent className="flex flex-col items-center justify-center py-20">
          <Database className="h-16 w-16 text-[--color-dim]" />
          <p className="mt-4 text-lg text-[--color-ghost]">Ledger explorer coming soon</p>
          <p className="mt-2 text-sm text-[--color-dim]">
            Browse blocks and transactions from the local blockchain
          </p>
        </CardContent>
      </Card>
    </Layout>
  )
}
