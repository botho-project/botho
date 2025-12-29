import { Layout } from '@/components/layout'
import { StatsCard, PulseChart, RecentBlocks, NetworkStatus } from '@/components/dashboard'
import { Blocks, Activity, Cpu, Coins } from 'lucide-react'

export function DashboardPage() {
  return (
    <Layout title="Dashboard" subtitle="Real-time blockchain overview">
      {/* Stats Grid */}
      <div className="grid grid-cols-4 gap-4">
        <StatsCard
          title="Block Height"
          value="1,234,567"
          icon={Blocks}
          color="pulse"
          delay={0}
        />
        <StatsCard
          title="Transactions"
          value="45.2M"
          change={{ value: 12.5, label: 'last 24h' }}
          icon={Activity}
          color="success"
          delay={0.1}
        />
        <StatsCard
          title="Hash Rate"
          value="1.2 GH/s"
          change={{ value: 5.3, label: 'last hour' }}
          icon={Cpu}
          color="purple"
          delay={0.2}
        />
        <StatsCard
          title="Total Supply"
          value="18.5M CAD"
          icon={Coins}
          color="warning"
          delay={0.3}
        />
      </div>

      {/* Main content grid */}
      <div className="mt-6 grid grid-cols-3 gap-6">
        {/* Left column - 2/3 width */}
        <div className="col-span-2 space-y-6">
          <PulseChart />
          <RecentBlocks />
        </div>

        {/* Right column - 1/3 width */}
        <div className="space-y-6">
          <NetworkStatus />

          {/* Mining Summary */}
          <div className="rounded-xl border border-[--color-steel] bg-[--color-abyss]/80 p-5">
            <div className="mb-4 flex items-center justify-between">
              <h3 className="font-display text-sm font-semibold uppercase tracking-wider text-[--color-soft]">
                Mining Summary
              </h3>
              <span className="rounded-full bg-[--color-success]/10 px-2 py-0.5 text-xs font-medium text-[--color-success]">
                Active
              </span>
            </div>

            <div className="space-y-4">
              <div>
                <div className="mb-1 flex justify-between text-sm">
                  <span className="text-[--color-dim]">Difficulty</span>
                  <span className="font-mono text-[--color-light]">2.45T</span>
                </div>
                <div className="h-1.5 overflow-hidden rounded-full bg-[--color-slate]">
                  <div
                    className="h-full rounded-full bg-gradient-to-r from-[--color-pulse] to-[--color-purple]"
                    style={{ width: '67%' }}
                  />
                </div>
              </div>

              <div>
                <div className="mb-1 flex justify-between text-sm">
                  <span className="text-[--color-dim]">Block Reward</span>
                  <span className="font-mono text-[--color-light]">2.5 CAD</span>
                </div>
              </div>

              <div>
                <div className="mb-1 flex justify-between text-sm">
                  <span className="text-[--color-dim]">Next Halving</span>
                  <span className="font-mono text-[--color-light]">~234 days</span>
                </div>
              </div>

              <div>
                <div className="mb-1 flex justify-between text-sm">
                  <span className="text-[--color-dim]">Avg Block Time</span>
                  <span className="font-mono text-[--color-light]">60s</span>
                </div>
              </div>
            </div>
          </div>

          {/* Fee Distribution */}
          <div className="rounded-xl border border-[--color-steel] bg-[--color-abyss]/80 p-5">
            <h3 className="mb-4 font-display text-sm font-semibold uppercase tracking-wider text-[--color-soft]">
              Fee Distribution (24h)
            </h3>
            <div className="space-y-3">
              <div className="flex items-center gap-3">
                <div className="h-3 w-3 rounded-full bg-[--color-pulse]" />
                <span className="flex-1 text-sm text-[--color-dim]">Plain (0.05%)</span>
                <span className="font-mono text-sm text-[--color-light]">45%</span>
              </div>
              <div className="flex items-center gap-3">
                <div className="h-3 w-3 rounded-full bg-[--color-purple]" />
                <span className="flex-1 text-sm text-[--color-dim]">Hidden (0.2-1.2%)</span>
                <span className="font-mono text-sm text-[--color-light]">52%</span>
              </div>
              <div className="flex items-center gap-3">
                <div className="h-3 w-3 rounded-full bg-[--color-success]" />
                <span className="flex-1 text-sm text-[--color-dim]">Mining</span>
                <span className="font-mono text-sm text-[--color-light]">3%</span>
              </div>
            </div>

            {/* Visual bar */}
            <div className="mt-4 flex h-2 overflow-hidden rounded-full">
              <div className="h-full bg-[--color-pulse]" style={{ width: '45%' }} />
              <div className="h-full bg-[--color-purple]" style={{ width: '52%' }} />
              <div className="h-full bg-[--color-success]" style={{ width: '3%' }} />
            </div>
          </div>
        </div>
      </div>
    </Layout>
  )
}
