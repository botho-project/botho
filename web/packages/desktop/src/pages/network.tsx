import { Layout } from '../components/layout'
import { Card, CardContent } from '@botho/ui'
import { Network } from 'lucide-react'

export function NetworkPage() {
  return (
    <Layout title="Network" subtitle="Peer connections and topology">
      <Card>
        <CardContent className="flex flex-col items-center justify-center py-20">
          <Network className="h-16 w-16 text-[--color-dim]" />
          <p className="mt-4 text-lg text-[--color-ghost]">Network topology view coming soon</p>
          <p className="mt-2 text-sm text-[--color-dim]">
            Visualize peer connections and network health
          </p>
        </CardContent>
      </Card>
    </Layout>
  )
}
