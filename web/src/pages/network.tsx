import { Layout } from '@/components/layout'
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card'
import { motion } from 'motion/react'
import {
  Activity,
  CheckCircle2,
  Circle,
  Clock,
  Globe,
  Loader2,
  Network,
  Radio,
  Server,
  Shield,
  Signal,
  Users,
  Wifi,
  Zap,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { cn } from '@/lib/utils'

interface SCPNode {
  id: string
  name: string
  address: string
  status: 'online' | 'syncing' | 'offline'
  latency: number
  slot: number
  phase: 'nominate' | 'prepare' | 'commit' | 'externalize'
  isValidator: boolean
  quorumSet: string[]
}

interface SCPSlot {
  slot: number
  phase: 'nominate' | 'prepare' | 'commit' | 'externalize'
  startTime: number
  votes: number
  totalNodes: number
}

const mockNodes: SCPNode[] = [
  { id: 'node-1', name: 'US-East Validator', address: '10.0.1.1:8443', status: 'online', latency: 12, slot: 1234567, phase: 'externalize', isValidator: true, quorumSet: ['node-2', 'node-3', 'node-4'] },
  { id: 'node-2', name: 'EU-West Validator', address: '10.0.2.1:8443', status: 'online', latency: 45, slot: 1234567, phase: 'externalize', isValidator: true, quorumSet: ['node-1', 'node-3', 'node-5'] },
  { id: 'node-3', name: 'Asia-Pacific Node', address: '10.0.3.1:8443', status: 'syncing', latency: 89, slot: 1234566, phase: 'commit', isValidator: true, quorumSet: ['node-1', 'node-2', 'node-4'] },
  { id: 'node-4', name: 'US-West Node', address: '10.0.4.1:8443', status: 'online', latency: 23, slot: 1234567, phase: 'externalize', isValidator: false, quorumSet: ['node-1', 'node-2'] },
  { id: 'node-5', name: 'SA-East Node', address: '10.0.5.1:8443', status: 'online', latency: 67, slot: 1234567, phase: 'externalize', isValidator: false, quorumSet: ['node-2', 'node-3'] },
  { id: 'node-6', name: 'AF-South Node', address: '10.0.6.1:8443', status: 'offline', latency: 0, slot: 1234560, phase: 'nominate', isValidator: false, quorumSet: ['node-1', 'node-4'] },
]

const mockSlotHistory: SCPSlot[] = [
  { slot: 1234567, phase: 'externalize', startTime: Date.now() - 5000, votes: 5, totalNodes: 6 },
  { slot: 1234566, phase: 'externalize', startTime: Date.now() - 65000, votes: 6, totalNodes: 6 },
  { slot: 1234565, phase: 'externalize', startTime: Date.now() - 125000, votes: 5, totalNodes: 6 },
  { slot: 1234564, phase: 'externalize', startTime: Date.now() - 185000, votes: 6, totalNodes: 6 },
  { slot: 1234563, phase: 'externalize', startTime: Date.now() - 245000, votes: 6, totalNodes: 6 },
]

const phaseColors = {
  nominate: { bg: 'bg-[--color-warning]/20', text: 'text-[--color-warning]', label: 'Nominate' },
  prepare: { bg: 'bg-[--color-pulse]/20', text: 'text-[--color-pulse]', label: 'Prepare' },
  commit: { bg: 'bg-[--color-purple]/20', text: 'text-[--color-purple]', label: 'Commit' },
  externalize: { bg: 'bg-[--color-success]/20', text: 'text-[--color-success]', label: 'Externalize' },
}

const statusColors = {
  online: 'bg-[--color-success]',
  syncing: 'bg-[--color-warning]',
  offline: 'bg-[--color-danger]',
}

export function NetworkPage() {
  const [currentSlot, setCurrentSlot] = useState(1234567)
  const [currentPhase, setCurrentPhase] = useState<keyof typeof phaseColors>('externalize')

  // Simulate SCP phase progression
  useEffect(() => {
    const phases: (keyof typeof phaseColors)[] = ['nominate', 'prepare', 'commit', 'externalize']
    let phaseIndex = 3

    const interval = setInterval(() => {
      phaseIndex = (phaseIndex + 1) % 4
      setCurrentPhase(phases[phaseIndex])
      if (phaseIndex === 0) {
        setCurrentSlot((s) => s + 1)
      }
    }, 3000)

    return () => clearInterval(interval)
  }, [])

  const onlineNodes = mockNodes.filter((n) => n.status === 'online').length
  const validators = mockNodes.filter((n) => n.isValidator).length
  const avgLatency = Math.round(
    mockNodes.filter((n) => n.status === 'online').reduce((sum, n) => sum + n.latency, 0) / onlineNodes
  )

  return (
    <Layout title="Network" subtitle="SCP consensus and network topology">
      <div className="space-y-6">
        {/* Live consensus status */}
        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          className="relative overflow-hidden rounded-2xl border border-[--color-steel] bg-gradient-to-br from-[--color-abyss] to-[--color-slate] p-6"
        >
          <div className="absolute -right-20 -top-20 h-64 w-64 rounded-full bg-[--color-pulse]/5 blur-3xl" />

          <div className="relative flex items-center justify-between">
            <div>
              <div className="flex items-center gap-3">
                <div className="relative">
                  <Radio className="h-6 w-6 text-[--color-pulse]" />
                  <span className="absolute -right-1 -top-1 flex h-3 w-3">
                    <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-[--color-pulse] opacity-75" />
                    <span className="relative inline-flex h-3 w-3 rounded-full bg-[--color-pulse]" />
                  </span>
                </div>
                <h2 className="font-display text-xl font-bold text-[--color-light]">
                  SCP Consensus Live
                </h2>
              </div>
              <p className="mt-2 text-sm text-[--color-dim]">
                Stellar Consensus Protocol is actively processing slot {currentSlot.toLocaleString()}
              </p>
            </div>

            {/* Current phase indicator */}
            <div className="flex items-center gap-6">
              {(['nominate', 'prepare', 'commit', 'externalize'] as const).map((phase, i) => {
                const isActive = currentPhase === phase
                const isPast = ['nominate', 'prepare', 'commit', 'externalize'].indexOf(currentPhase) > i
                const color = phaseColors[phase]

                return (
                  <div key={phase} className="flex items-center">
                    <div className="flex flex-col items-center">
                      <motion.div
                        animate={{
                          scale: isActive ? [1, 1.2, 1] : 1,
                          opacity: isActive || isPast ? 1 : 0.3,
                        }}
                        transition={{ duration: 0.5, repeat: isActive ? Infinity : 0 }}
                        className={cn(
                          'flex h-10 w-10 items-center justify-center rounded-full border-2',
                          isActive
                            ? `${color.bg} border-current ${color.text}`
                            : isPast
                              ? 'border-[--color-success] bg-[--color-success]/20'
                              : 'border-[--color-steel] bg-[--color-slate]'
                        )}
                      >
                        {isPast && !isActive ? (
                          <CheckCircle2 className="h-5 w-5 text-[--color-success]" />
                        ) : isActive ? (
                          <Loader2 className={cn('h-5 w-5 animate-spin', color.text)} />
                        ) : (
                          <Circle className="h-5 w-5 text-[--color-dim]" />
                        )}
                      </motion.div>
                      <span className={cn(
                        'mt-1 text-xs font-medium capitalize',
                        isActive ? color.text : isPast ? 'text-[--color-success]' : 'text-[--color-dim]'
                      )}>
                        {phase}
                      </span>
                    </div>
                    {i < 3 && (
                      <div className={cn(
                        'mx-2 h-0.5 w-8',
                        isPast ? 'bg-[--color-success]' : 'bg-[--color-steel]'
                      )} />
                    )}
                  </div>
                )
              })}
            </div>
          </div>
        </motion.div>

        {/* Stats grid */}
        <div className="grid grid-cols-5 gap-4">
          {[
            { label: 'Connected Peers', value: onlineNodes, total: mockNodes.length, icon: Users, color: 'success' },
            { label: 'Validators', value: validators, icon: Shield, color: 'purple' },
            { label: 'Current Slot', value: currentSlot.toLocaleString(), icon: Zap, color: 'pulse' },
            { label: 'Avg Latency', value: `${avgLatency}ms`, icon: Signal, color: 'warning' },
            { label: 'Ledger Close', value: '~60s', icon: Clock, color: 'pulse' },
          ].map((stat, i) => (
            <motion.div
              key={stat.label}
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: i * 0.05 }}
              className="rounded-xl border border-[--color-steel] bg-[--color-abyss]/80 p-4"
            >
              <div className="flex items-center gap-2">
                <stat.icon className={cn('h-4 w-4', `text-[--color-${stat.color}]`)} />
                <span className="text-xs text-[--color-dim]">{stat.label}</span>
              </div>
              <p className="mt-1 font-display text-xl font-bold text-[--color-light]">
                {stat.value}
                {'total' in stat && (
                  <span className="ml-1 text-sm text-[--color-dim]">/ {stat.total}</span>
                )}
              </p>
            </motion.div>
          ))}
        </div>

        <div className="grid grid-cols-3 gap-6">
          {/* Node list */}
          <div className="col-span-2">
            <Card>
              <CardHeader>
                <div className="flex items-center gap-2">
                  <Server className="h-4 w-4 text-[--color-pulse]" />
                  <CardTitle>Network Nodes</CardTitle>
                </div>
                <div className="flex gap-2">
                  <span className="flex items-center gap-1 text-xs text-[--color-dim]">
                    <div className="h-2 w-2 rounded-full bg-[--color-success]" /> Online
                  </span>
                  <span className="flex items-center gap-1 text-xs text-[--color-dim]">
                    <div className="h-2 w-2 rounded-full bg-[--color-warning]" /> Syncing
                  </span>
                  <span className="flex items-center gap-1 text-xs text-[--color-dim]">
                    <div className="h-2 w-2 rounded-full bg-[--color-danger]" /> Offline
                  </span>
                </div>
              </CardHeader>
              <CardContent className="p-0">
                <div className="divide-y divide-[--color-steel]">
                  {mockNodes.map((node, i) => {
                    const phase = phaseColors[node.phase]

                    return (
                      <motion.div
                        key={node.id}
                        initial={{ opacity: 0, x: -20 }}
                        animate={{ opacity: 1, x: 0 }}
                        transition={{ delay: i * 0.05 }}
                        className="group flex items-center justify-between px-5 py-4 transition-colors hover:bg-[--color-slate]/50"
                      >
                        <div className="flex items-center gap-4">
                          <div className="relative">
                            <div className={cn(
                              'flex h-10 w-10 items-center justify-center rounded-lg',
                              node.isValidator ? 'bg-[--color-purple]/10' : 'bg-[--color-slate]'
                            )}>
                              {node.isValidator ? (
                                <Shield className="h-5 w-5 text-[--color-purple]" />
                              ) : (
                                <Server className="h-5 w-5 text-[--color-dim]" />
                              )}
                            </div>
                            <div className={cn(
                              'absolute -bottom-0.5 -right-0.5 h-3 w-3 rounded-full border-2 border-[--color-abyss]',
                              statusColors[node.status]
                            )} />
                          </div>
                          <div>
                            <div className="flex items-center gap-2">
                              <span className="font-medium text-[--color-light]">{node.name}</span>
                              {node.isValidator && (
                                <span className="rounded bg-[--color-purple]/10 px-1.5 py-0.5 text-xs font-medium text-[--color-purple]">
                                  Validator
                                </span>
                              )}
                            </div>
                            <div className="mt-0.5 flex items-center gap-2 text-xs text-[--color-dim]">
                              <span className="font-mono">{node.address}</span>
                              <span>•</span>
                              <span>Slot {node.slot.toLocaleString()}</span>
                            </div>
                          </div>
                        </div>
                        <div className="flex items-center gap-4">
                          <div className={cn('rounded px-2 py-1 text-xs font-medium', phase.bg, phase.text)}>
                            {phase.label}
                          </div>
                          <div className="w-16 text-right">
                            {node.status === 'online' ? (
                              <span className={cn(
                                'font-mono text-sm',
                                node.latency < 50 ? 'text-[--color-success]' : node.latency < 100 ? 'text-[--color-warning]' : 'text-[--color-danger]'
                              )}>
                                {node.latency}ms
                              </span>
                            ) : (
                              <span className="text-xs text-[--color-dim]">{node.status}</span>
                            )}
                          </div>
                        </div>
                      </motion.div>
                    )
                  })}
                </div>
              </CardContent>
            </Card>
          </div>

          {/* Right column */}
          <div className="space-y-6">
            {/* Slot history */}
            <Card>
              <CardHeader>
                <div className="flex items-center gap-2">
                  <Activity className="h-4 w-4 text-[--color-success]" />
                  <CardTitle>Recent Slots</CardTitle>
                </div>
              </CardHeader>
              <CardContent className="space-y-3">
                {mockSlotHistory.map((slot, i) => (
                  <motion.div
                    key={slot.slot}
                    initial={{ opacity: 0, x: 10 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ delay: i * 0.05 }}
                    className="flex items-center justify-between rounded-lg bg-[--color-slate]/50 px-3 py-2"
                  >
                    <div className="flex items-center gap-2">
                      <CheckCircle2 className="h-4 w-4 text-[--color-success]" />
                      <span className="font-mono text-sm text-[--color-light]">
                        #{slot.slot.toLocaleString()}
                      </span>
                    </div>
                    <div className="text-right">
                      <span className="text-xs text-[--color-success]">
                        {slot.votes}/{slot.totalNodes} votes
                      </span>
                    </div>
                  </motion.div>
                ))}
              </CardContent>
            </Card>

            {/* Quorum info */}
            <Card>
              <CardHeader>
                <div className="flex items-center gap-2">
                  <Globe className="h-4 w-4 text-[--color-purple]" />
                  <CardTitle>Quorum Status</CardTitle>
                </div>
              </CardHeader>
              <CardContent className="space-y-4">
                <div>
                  <div className="mb-1 flex justify-between text-sm">
                    <span className="text-[--color-dim]">Quorum Threshold</span>
                    <span className="text-[--color-light]">67%</span>
                  </div>
                  <div className="h-2 overflow-hidden rounded-full bg-[--color-slate]">
                    <motion.div
                      initial={{ width: 0 }}
                      animate={{ width: '83%' }}
                      transition={{ duration: 1, delay: 0.5 }}
                      className="h-full rounded-full bg-gradient-to-r from-[--color-pulse] to-[--color-success]"
                    />
                  </div>
                  <p className="mt-1 text-xs text-[--color-success]">83% agreement reached</p>
                </div>

                <div className="space-y-2">
                  <p className="text-xs text-[--color-dim]">Active Validators</p>
                  <div className="flex flex-wrap gap-1">
                    {mockNodes.filter((n) => n.isValidator).map((node) => (
                      <div
                        key={node.id}
                        className={cn(
                          'rounded px-2 py-1 text-xs',
                          node.status === 'online'
                            ? 'bg-[--color-success]/10 text-[--color-success]'
                            : 'bg-[--color-danger]/10 text-[--color-danger]'
                        )}
                      >
                        {node.name.split(' ')[0]}
                      </div>
                    ))}
                  </div>
                </div>

                <div className="rounded-lg bg-[--color-slate] p-3">
                  <div className="flex items-center gap-2">
                    <Wifi className="h-4 w-4 text-[--color-pulse]" />
                    <span className="text-sm font-medium text-[--color-light]">Network Health</span>
                  </div>
                  <p className="mt-1 text-xs text-[--color-success]">
                    All quorum intersections intact. Network is fully connected.
                  </p>
                </div>
              </CardContent>
            </Card>

            {/* Connection info */}
            <Card>
              <CardHeader>
                <div className="flex items-center gap-2">
                  <Network className="h-4 w-4 text-[--color-warning]" />
                  <CardTitle>Your Connection</CardTitle>
                </div>
              </CardHeader>
              <CardContent className="space-y-3">
                <div className="flex justify-between text-sm">
                  <span className="text-[--color-dim]">Connected To</span>
                  <span className="text-[--color-light]">localhost:8443</span>
                </div>
                <div className="flex justify-between text-sm">
                  <span className="text-[--color-dim]">Node Type</span>
                  <span className="text-[--color-purple]">Validator</span>
                </div>
                <div className="flex justify-between text-sm">
                  <span className="text-[--color-dim]">Peer Count</span>
                  <span className="text-[--color-light]">{onlineNodes - 1}</span>
                </div>
                <div className="flex justify-between text-sm">
                  <span className="text-[--color-dim]">Sync Status</span>
                  <span className="text-[--color-success]">Synchronized</span>
                </div>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </Layout>
  )
}
