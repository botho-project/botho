import { Layout } from '../components/layout'
import { Card, CardHeader, CardTitle, CardContent } from '@botho/ui'
import { motion } from 'motion/react'
import {
  Blocks,
  Clock,
  Cpu,
  TrendingUp,
  Users,
  Wallet,
} from 'lucide-react'

const stats = [
  { label: 'Block Height', value: '1,234,567', icon: Blocks, trend: '+12' },
  { label: 'Hash Rate', value: '45.2 MH/s', icon: Cpu, trend: '+5.2%' },
  { label: 'Connected Peers', value: '24', icon: Users, trend: null },
  { label: 'Wallet Balance', value: '1,234.56 BTH', icon: Wallet, trend: '+125.50' },
]

export function DashboardPage() {
  return (
    <Layout title="Dashboard" subtitle="Network overview">
      <div className="space-y-6">
        {/* Stats grid */}
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          {stats.map((stat, i) => (
            <motion.div
              key={stat.label}
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: i * 0.1 }}
            >
              <Card>
                <CardContent className="flex items-center gap-4">
                  <div className="flex h-12 w-12 items-center justify-center rounded-lg bg-[--color-pulse]/10">
                    <stat.icon className="h-6 w-6 text-[--color-pulse]" />
                  </div>
                  <div>
                    <p className="text-sm text-[--color-dim]">{stat.label}</p>
                    <p className="font-display text-xl font-bold text-[--color-light]">
                      {stat.value}
                    </p>
                    {stat.trend && (
                      <p className="flex items-center gap-1 text-xs text-[--color-success]">
                        <TrendingUp className="h-3 w-3" />
                        {stat.trend}
                      </p>
                    )}
                  </div>
                </CardContent>
              </Card>
            </motion.div>
          ))}
        </div>

        {/* Recent blocks */}
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Clock className="h-4 w-4 text-[--color-pulse]" />
              <CardTitle>Recent Blocks</CardTitle>
            </div>
          </CardHeader>
          <CardContent>
            <div className="text-center py-12 text-[--color-dim]">
              Block data will appear here when connected to the network
            </div>
          </CardContent>
        </Card>
      </div>
    </Layout>
  )
}
