import { Layout } from '../components/layout'
import { Card, CardHeader, CardTitle, CardContent } from '@botho/ui'
import { NetworkGraph, type NetworkPeer } from '../components/network'
import { motion, AnimatePresence } from 'motion/react'
import {
  Activity,
  CheckCircle2,
  Circle,
  Clock,
  Globe,
  Loader2,
  Radio,
  Server,
  Shield,
  Signal,
  Users,
  X,
  Zap,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { cn } from '@botho/ui'

const phaseColors = {
  nominate: { bg: 'bg-[--color-warning]/20', text: 'text-[--color-warning]', label: 'Nominate' },
  prepare: { bg: 'bg-[--color-pulse]/20', text: 'text-[--color-pulse]', label: 'Prepare' },
  commit: { bg: 'bg-[--color-purple]/20', text: 'text-[--color-purple]', label: 'Commit' },
  externalize: { bg: 'bg-[--color-success]/20', text: 'text-[--color-success]', label: 'Externalize' },
}

// Mock peer data - in production this comes from gossip
const mockPeers: NetworkPeer[] = [
  { id: 'self', nodeId: 'bt1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh', isValidator: true, isSelf: true, status: 'online', latency: 0, blockHeight: 1234567, version: '0.1.0', connectedTo: ['node-2', 'node-3', 'node-4', 'node-5'] },
  { id: 'node-2', nodeId: 'bt1pqr8stuvwxyz0123456789abcdefghijklmnop', isValidator: true, isSelf: false, status: 'online', latency: 45, blockHeight: 1234567, version: '0.1.0', connectedTo: ['self', 'node-3', 'node-5', 'node-6'] },
  { id: 'node-3', nodeId: 'bt1abc9defghijk0123456789lmnopqrstuvwxyz', isValidator: true, isSelf: false, status: 'online', latency: 89, blockHeight: 1234566, version: '0.1.0', connectedTo: ['self', 'node-2', 'node-4', 'node-7'] },
  { id: 'node-4', nodeId: 'bt1xyz7890abcdefghijklmnopqrstuvwxyz0123', isValidator: false, isSelf: false, status: 'online', latency: 23, blockHeight: 1234567, version: '0.1.0', connectedTo: ['self', 'node-3', 'node-6'] },
  { id: 'node-5', nodeId: 'bt1lmn4567opqrstuvwxyzabcdefghijk890123', isValidator: false, isSelf: false, status: 'syncing', latency: 67, blockHeight: 1234560, version: '0.1.0', connectedTo: ['self', 'node-2', 'node-7'] },
  { id: 'node-6', nodeId: 'bt1def1234ghijklmnopqrstuvwxyzabc567890', isValidator: false, isSelf: false, status: 'offline', latency: 0, blockHeight: 1234550, version: '0.0.9', connectedTo: ['node-2', 'node-4'] },
  { id: 'node-7', nodeId: 'bt1ghi8901jklmnopqrstuvwxyzabcdef234567', isValidator: true, isSelf: false, status: 'online', latency: 112, blockHeight: 1234567, version: '0.1.0', connectedTo: ['node-3', 'node-5', 'node-8'] },
  { id: 'node-8', nodeId: 'bt1jkl5678mnopqrstuvwxyzabcdefghi901234', isValidator: false, isSelf: false, status: 'online', latency: 156, blockHeight: 1234567, version: '0.1.0', connectedTo: ['node-7'] },
]

interface SCPSlot {
  slot: number
  phase: 'nominate' | 'prepare' | 'commit' | 'externalize'
  startTime: number
  votes: number
  totalNodes: number
}

const mockSlotHistory: SCPSlot[] = [
  { slot: 1234567, phase: 'externalize', startTime: Date.now() - 5000, votes: 5, totalNodes: 6 },
  { slot: 1234566, phase: 'externalize', startTime: Date.now() - 65000, votes: 6, totalNodes: 6 },
  { slot: 1234565, phase: 'externalize', startTime: Date.now() - 125000, votes: 5, totalNodes: 6 },
  { slot: 1234564, phase: 'externalize', startTime: Date.now() - 185000, votes: 6, totalNodes: 6 },
]

export function NetworkPage() {
  const [currentSlot, setCurrentSlot] = useState(1234567)
  const [currentPhase, setCurrentPhase] = useState<keyof typeof phaseColors>('externalize')
  const [selectedPeer, setSelectedPeer] = useState<NetworkPeer | null>(null)

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

  const onlinePeers = mockPeers.filter((p) => p.status === 'online').length
  const validators = mockPeers.filter((p) => p.isValidator).length
  const avgLatency = Math.round(
    mockPeers.filter((p) => p.status === 'online' && !p.isSelf).reduce((sum, p) => sum + (p.latency || 0), 0) / (onlinePeers - 1)
  )

  return (
    <Layout title="Network" subtitle="P2P gossip topology and SCP consensus">
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
                Processing slot {currentSlot.toLocaleString()}
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
            { label: 'Known Peers', value: mockPeers.length - 1, icon: Users, color: 'success' },
            { label: 'Validators', value: validators, icon: Shield, color: 'purple' },
            { label: 'Current Slot', value: currentSlot.toLocaleString(), icon: Zap, color: 'pulse' },
            { label: 'Avg Latency', value: `${avgLatency}ms`, icon: Signal, color: 'warning' },
            { label: 'Slot Time', value: '~60s', icon: Clock, color: 'pulse' },
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
              </p>
            </motion.div>
          ))}
        </div>

        {/* Network graph and side panel */}
        <div className="grid grid-cols-4 gap-6">
          {/* Network graph */}
          <div className="col-span-3">
            <Card className="h-[600px] overflow-hidden">
              <CardHeader className="border-b border-[--color-steel]">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <Globe className="h-4 w-4 text-[--color-pulse]" />
                    <CardTitle>Network Topology</CardTitle>
                  </div>
                  <div className="flex gap-4">
                    <span className="flex items-center gap-1.5 text-xs text-[--color-dim]">
                      <div className="h-2 w-2 rounded-full bg-[--color-pulse]" /> You
                    </span>
                    <span className="flex items-center gap-1.5 text-xs text-[--color-dim]">
                      <div className="h-2 w-2 rounded-full bg-[--color-purple]" /> Validator
                    </span>
                    <span className="flex items-center gap-1.5 text-xs text-[--color-dim]">
                      <div className="h-2 w-2 rounded-full bg-[--color-steel]" /> Node
                    </span>
                  </div>
                </div>
              </CardHeader>
              <CardContent className="h-[calc(100%-65px)] p-0">
                <NetworkGraph peers={mockPeers} onPeerSelect={setSelectedPeer} />
              </CardContent>
            </Card>
          </div>

          {/* Side panel */}
          <div className="space-y-6">
            {/* Selected peer details or recent slots */}
            <AnimatePresence mode="wait">
              {selectedPeer ? (
                <motion.div
                  key="peer-details"
                  initial={{ opacity: 0, x: 20 }}
                  animate={{ opacity: 1, x: 0 }}
                  exit={{ opacity: 0, x: 20 }}
                >
                  <Card>
                    <CardHeader>
                      <div className="flex items-center justify-between">
                        <div className="flex items-center gap-2">
                          {selectedPeer.isSelf ? (
                            <Radio className="h-4 w-4 text-[--color-pulse]" />
                          ) : selectedPeer.isValidator ? (
                            <Shield className="h-4 w-4 text-[--color-purple]" />
                          ) : (
                            <Server className="h-4 w-4 text-[--color-dim]" />
                          )}
                          <CardTitle>
                            {selectedPeer.isSelf ? 'Your Node' : 'Peer Details'}
                          </CardTitle>
                        </div>
                        <button
                          onClick={() => setSelectedPeer(null)}
                          className="rounded p-1 text-[--color-dim] hover:bg-[--color-slate] hover:text-[--color-light]"
                        >
                          <X className="h-4 w-4" />
                        </button>
                      </div>
                    </CardHeader>
                    <CardContent className="space-y-4">
                      <div>
                        <p className="text-xs text-[--color-dim]">Node ID</p>
                        <p className="mt-1 break-all font-mono text-xs text-[--color-light]">
                          {selectedPeer.nodeId}
                        </p>
                      </div>

                      <div className="grid grid-cols-2 gap-4">
                        <div>
                          <p className="text-xs text-[--color-dim]">Status</p>
                          <p className={cn(
                            'mt-1 text-sm font-medium capitalize',
                            selectedPeer.status === 'online' ? 'text-[--color-success]' :
                            selectedPeer.status === 'syncing' ? 'text-[--color-warning]' :
                            'text-[--color-danger]'
                          )}>
                            {selectedPeer.status}
                          </p>
                        </div>
                        <div>
                          <p className="text-xs text-[--color-dim]">Role</p>
                          <p className={cn(
                            'mt-1 text-sm font-medium',
                            selectedPeer.isValidator ? 'text-[--color-purple]' : 'text-[--color-ghost]'
                          )}>
                            {selectedPeer.isValidator ? 'Validator' : 'Node'}
                          </p>
                        </div>
                      </div>

                      {!selectedPeer.isSelf && selectedPeer.latency !== undefined && (
                        <div>
                          <p className="text-xs text-[--color-dim]">Latency</p>
                          <p className={cn(
                            'mt-1 font-mono text-sm',
                            selectedPeer.latency < 50 ? 'text-[--color-success]' :
                            selectedPeer.latency < 100 ? 'text-[--color-warning]' :
                            'text-[--color-danger]'
                          )}>
                            {selectedPeer.latency}ms
                          </p>
                        </div>
                      )}

                      <div>
                        <p className="text-xs text-[--color-dim]">Block Height</p>
                        <p className="mt-1 font-mono text-sm text-[--color-light]">
                          {selectedPeer.blockHeight?.toLocaleString() || 'Unknown'}
                        </p>
                      </div>

                      <div>
                        <p className="text-xs text-[--color-dim]">Version</p>
                        <p className="mt-1 text-sm text-[--color-ghost]">
                          v{selectedPeer.version || 'Unknown'}
                        </p>
                      </div>

                      <div>
                        <p className="text-xs text-[--color-dim]">Connected Peers</p>
                        <p className="mt-1 text-sm text-[--color-light]">
                          {selectedPeer.connectedTo.length}
                        </p>
                      </div>
                    </CardContent>
                  </Card>
                </motion.div>
              ) : (
                <motion.div
                  key="slot-history"
                  initial={{ opacity: 0, x: 20 }}
                  animate={{ opacity: 1, x: 0 }}
                  exit={{ opacity: 0, x: 20 }}
                >
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
                              {slot.votes}/{slot.totalNodes}
                            </span>
                          </div>
                        </motion.div>
                      ))}
                    </CardContent>
                  </Card>
                </motion.div>
              )}
            </AnimatePresence>

            {/* Quorum info */}
            <Card>
              <CardHeader>
                <div className="flex items-center gap-2">
                  <Shield className="h-4 w-4 text-[--color-purple]" />
                  <CardTitle>Quorum Status</CardTitle>
                </div>
              </CardHeader>
              <CardContent className="space-y-4">
                <div>
                  <div className="mb-1 flex justify-between text-sm">
                    <span className="text-[--color-dim]">Agreement</span>
                    <span className="text-[--color-light]">83%</span>
                  </div>
                  <div className="h-2 overflow-hidden rounded-full bg-[--color-slate]">
                    <motion.div
                      initial={{ width: 0 }}
                      animate={{ width: '83%' }}
                      transition={{ duration: 1, delay: 0.5 }}
                      className="h-full rounded-full bg-gradient-to-r from-[--color-pulse] to-[--color-success]"
                    />
                  </div>
                </div>

                <div className="space-y-2">
                  <p className="text-xs text-[--color-dim]">Active Validators</p>
                  <div className="flex flex-wrap gap-1">
                    {mockPeers.filter((p) => p.isValidator).map((peer) => (
                      <div
                        key={peer.id}
                        className={cn(
                          'rounded px-2 py-1 text-xs cursor-pointer transition-colors',
                          peer.status === 'online'
                            ? 'bg-[--color-success]/10 text-[--color-success] hover:bg-[--color-success]/20'
                            : 'bg-[--color-danger]/10 text-[--color-danger]'
                        )}
                        onClick={() => setSelectedPeer(peer)}
                      >
                        {peer.isSelf ? 'You' : peer.nodeId.slice(0, 8)}
                      </div>
                    ))}
                  </div>
                </div>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </Layout>
  )
}
