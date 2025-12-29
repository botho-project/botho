import { motion } from 'motion/react'
import { Globe, Server, Users, Zap } from 'lucide-react'
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card'
import { cn } from '@/lib/utils'

interface NetworkNode {
  id: string
  location: string
  latency: number
  status: 'online' | 'syncing' | 'offline'
}

const mockNodes: NetworkNode[] = [
  { id: 'node-1', location: 'US East', latency: 12, status: 'online' },
  { id: 'node-2', location: 'EU West', latency: 45, status: 'online' },
  { id: 'node-3', location: 'Asia Pacific', latency: 89, status: 'syncing' },
  { id: 'node-4', location: 'US West', latency: 23, status: 'online' },
]

const statusColors = {
  online: 'bg-[--color-success]',
  syncing: 'bg-[--color-warning]',
  offline: 'bg-[--color-danger]',
}

export function NetworkStatus() {
  return (
    <Card>
      <CardHeader>
        <div className="flex items-center gap-2">
          <Globe className="h-4 w-4 text-[--color-purple]" />
          <CardTitle>Network Status</CardTitle>
        </div>
        <div className="flex items-center gap-1.5">
          <div className="h-2 w-2 rounded-full bg-[--color-success]" />
          <span className="text-xs text-[--color-success]">Healthy</span>
        </div>
      </CardHeader>
      <CardContent>
        {/* Network stats */}
        <div className="mb-4 grid grid-cols-3 gap-4">
          <div className="rounded-lg bg-[--color-slate] p-3 text-center">
            <Users className="mx-auto mb-1 h-4 w-4 text-[--color-pulse]" />
            <p className="font-display text-lg font-bold text-[--color-light]">1,247</p>
            <p className="text-xs text-[--color-dim]">Peers</p>
          </div>
          <div className="rounded-lg bg-[--color-slate] p-3 text-center">
            <Server className="mx-auto mb-1 h-4 w-4 text-[--color-purple]" />
            <p className="font-display text-lg font-bold text-[--color-light]">89</p>
            <p className="text-xs text-[--color-dim]">Nodes</p>
          </div>
          <div className="rounded-lg bg-[--color-slate] p-3 text-center">
            <Zap className="mx-auto mb-1 h-4 w-4 text-[--color-warning]" />
            <p className="font-display text-lg font-bold text-[--color-light]">24ms</p>
            <p className="text-xs text-[--color-dim]">Avg Latency</p>
          </div>
        </div>

        {/* Node list */}
        <div className="space-y-2">
          {mockNodes.map((node, i) => (
            <motion.div
              key={node.id}
              initial={{ opacity: 0, x: -10 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ duration: 0.3, delay: i * 0.05 }}
              className="flex items-center justify-between rounded-lg bg-[--color-slate]/50 px-3 py-2"
            >
              <div className="flex items-center gap-3">
                <div className={cn('h-2 w-2 rounded-full', statusColors[node.status])} />
                <div>
                  <p className="text-sm text-[--color-soft]">{node.location}</p>
                  <p className="text-xs text-[--color-dim]">{node.id}</p>
                </div>
              </div>
              <div className="text-right">
                <p className={cn(
                  'font-mono text-sm',
                  node.latency < 50 ? 'text-[--color-success]' : node.latency < 100 ? 'text-[--color-warning]' : 'text-[--color-danger]'
                )}>
                  {node.latency}ms
                </p>
                <p className="text-xs capitalize text-[--color-dim]">{node.status}</p>
              </div>
            </motion.div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
