import { motion, AnimatePresence } from 'motion/react'
import {
  ChevronRight,
  Database,
  Loader2,
  Plus,
  Radio,
  RefreshCw,
  Server,
  Signal,
  WifiOff,
} from 'lucide-react'
import { useState } from 'react'
import { useConnection, type CadenceNode } from '@/contexts/connection'
import { cn } from '@/lib/utils'

export function SplashScreen() {
  const {
    isScanning,
    discoveredNodes,
    error,
    scanForNodes,
    connectToNode,
    addCustomNode,
  } = useConnection()

  const [showCustom, setShowCustom] = useState(false)
  const [customHost, setCustomHost] = useState('localhost')
  const [customPort, setCustomPort] = useState('8080')
  const [isConnecting, setIsConnecting] = useState<string | null>(null)

  const handleConnect = async (node: CadenceNode) => {
    setIsConnecting(node.id)
    await connectToNode(node)
    setIsConnecting(null)
  }

  const handleAddCustom = async (e: React.FormEvent) => {
    e.preventDefault()
    const port = parseInt(customPort, 10)
    if (isNaN(port) || port < 1 || port > 65535) return
    await addCustomNode(customHost, port)
    setShowCustom(false)
  }

  return (
    <div className="fixed inset-0 flex items-center justify-center bg-[--color-void]">
      {/* Background effects */}
      <div className="absolute inset-0 overflow-hidden">
        <div className="absolute -left-1/4 top-1/4 h-96 w-96 rounded-full bg-[--color-pulse]/5 blur-3xl" />
        <div className="absolute -right-1/4 bottom-1/4 h-96 w-96 rounded-full bg-[--color-purple]/5 blur-3xl" />
        <div className="absolute left-1/2 top-1/2 h-64 w-64 -translate-x-1/2 -translate-y-1/2 rounded-full bg-[--color-pulse]/3 blur-3xl" />
      </div>

      {/* Grid pattern */}
      <div
        className="absolute inset-0 opacity-[0.02]"
        style={{
          backgroundImage: `
            linear-gradient(var(--color-steel) 1px, transparent 1px),
            linear-gradient(90deg, var(--color-steel) 1px, transparent 1px)
          `,
          backgroundSize: '50px 50px',
        }}
      />

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        className="relative z-10 w-full max-w-lg px-6"
      >
        {/* Logo */}
        <div className="mb-12 text-center">
          <motion.div
            initial={{ scale: 0.8, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            transition={{ delay: 0.1 }}
            className="mx-auto mb-6 h-20 w-20"
          >
            <svg viewBox="0 0 32 32" className="h-full w-full">
              <defs>
                <linearGradient id="splash-gradient" x1="0%" y1="0%" x2="100%" y2="100%">
                  <stop offset="0%" stopColor="var(--color-pulse)" />
                  <stop offset="100%" stopColor="var(--color-purple)" />
                </linearGradient>
              </defs>
              <circle
                cx="16"
                cy="16"
                r="14"
                fill="none"
                stroke="url(#splash-gradient)"
                strokeWidth="1.5"
              />
              <path
                d="M8 16 Q12 10, 16 16 T24 16"
                fill="none"
                stroke="url(#splash-gradient)"
                strokeWidth="2"
                strokeLinecap="round"
              >
                <animate
                  attributeName="d"
                  dur="2s"
                  repeatCount="indefinite"
                  values="M8 16 Q12 10, 16 16 T24 16;M8 16 Q12 22, 16 16 T24 16;M8 16 Q12 10, 16 16 T24 16"
                />
              </path>
            </svg>
          </motion.div>
          <motion.h1
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ delay: 0.2 }}
            className="font-display text-4xl font-bold tracking-tight text-gradient"
          >
            Cadence
          </motion.h1>
          <motion.p
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ delay: 0.3 }}
            className="mt-2 text-[--color-dim]"
          >
            Connect to a local node to continue
          </motion.p>
        </div>

        {/* Connection panel */}
        <motion.div
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.4 }}
          className="rounded-2xl border border-[--color-steel] bg-[--color-abyss]/80 p-6 backdrop-blur-xl"
        >
          {/* Header */}
          <div className="mb-4 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Radio className="h-4 w-4 text-[--color-pulse]" />
              <span className="font-display text-sm font-semibold text-[--color-soft]">
                Local Nodes
              </span>
            </div>
            <button
              onClick={() => scanForNodes()}
              disabled={isScanning}
              className="flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs text-[--color-ghost] transition-colors hover:bg-[--color-steel] hover:text-[--color-light] disabled:opacity-50"
            >
              <RefreshCw className={cn('h-3 w-3', isScanning && 'animate-spin')} />
              Rescan
            </button>
          </div>

          {/* Node list */}
          <div className="space-y-2">
            <AnimatePresence mode="wait">
              {isScanning && discoveredNodes.length === 0 ? (
                <motion.div
                  key="scanning"
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  className="flex flex-col items-center py-8"
                >
                  <Loader2 className="h-8 w-8 animate-spin text-[--color-pulse]" />
                  <p className="mt-3 text-sm text-[--color-dim]">Scanning for nodes...</p>
                </motion.div>
              ) : discoveredNodes.length === 0 ? (
                <motion.div
                  key="empty"
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  className="flex flex-col items-center py-8"
                >
                  <div className="flex h-12 w-12 items-center justify-center rounded-full bg-[--color-slate]">
                    <WifiOff className="h-6 w-6 text-[--color-dim]" />
                  </div>
                  <p className="mt-3 text-sm text-[--color-dim]">No nodes found</p>
                  <p className="mt-1 text-xs text-[--color-steel]">
                    Start a Cadence node or add one manually
                  </p>
                </motion.div>
              ) : (
                <motion.div
                  key="nodes"
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  className="space-y-2"
                >
                  {discoveredNodes.map((node, i) => (
                    <motion.button
                      key={node.id}
                      initial={{ opacity: 0, x: -10 }}
                      animate={{ opacity: 1, x: 0 }}
                      transition={{ delay: i * 0.05 }}
                      onClick={() => handleConnect(node)}
                      disabled={isConnecting !== null}
                      className="group flex w-full items-center gap-4 rounded-xl border border-[--color-steel] bg-[--color-slate]/50 p-4 text-left transition-all hover:border-[--color-pulse-dim] hover:bg-[--color-slate] disabled:opacity-50"
                    >
                      <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-abyss]">
                        <Server className="h-5 w-5 text-[--color-pulse]" />
                      </div>
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2">
                          <span className="font-mono text-sm font-medium text-[--color-light]">
                            {node.host}:{node.port}
                          </span>
                          <div className="flex items-center gap-1">
                            <Signal className="h-3 w-3 text-[--color-success]" />
                            <span className="text-xs text-[--color-success]">
                              {node.latency}ms
                            </span>
                          </div>
                        </div>
                        <div className="mt-1 flex items-center gap-3 text-xs text-[--color-dim]">
                          {node.version && <span>v{node.version}</span>}
                          {node.blockHeight && (
                            <span className="flex items-center gap-1">
                              <Database className="h-3 w-3" />
                              {node.blockHeight.toLocaleString()}
                            </span>
                          )}
                          {node.networkId && (
                            <span className="rounded bg-[--color-abyss] px-1.5 py-0.5">
                              {node.networkId}
                            </span>
                          )}
                        </div>
                      </div>
                      {isConnecting === node.id ? (
                        <Loader2 className="h-5 w-5 animate-spin text-[--color-pulse]" />
                      ) : (
                        <ChevronRight className="h-5 w-5 text-[--color-steel] transition-colors group-hover:text-[--color-pulse]" />
                      )}
                    </motion.button>
                  ))}
                </motion.div>
              )}
            </AnimatePresence>
          </div>

          {/* Custom node input */}
          <AnimatePresence>
            {showCustom ? (
              <motion.form
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                onSubmit={handleAddCustom}
                className="mt-4 overflow-hidden"
              >
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={customHost}
                    onChange={(e) => setCustomHost(e.target.value)}
                    placeholder="Host"
                    className="flex-1 rounded-lg border border-[--color-steel] bg-[--color-slate] px-3 py-2 font-mono text-sm text-[--color-light] placeholder:text-[--color-dim] focus:border-[--color-pulse] focus:outline-none"
                  />
                  <input
                    type="text"
                    value={customPort}
                    onChange={(e) => setCustomPort(e.target.value)}
                    placeholder="Port"
                    className="w-24 rounded-lg border border-[--color-steel] bg-[--color-slate] px-3 py-2 font-mono text-sm text-[--color-light] placeholder:text-[--color-dim] focus:border-[--color-pulse] focus:outline-none"
                  />
                  <button
                    type="submit"
                    disabled={isScanning}
                    className="rounded-lg bg-[--color-pulse] px-4 py-2 font-display text-sm font-semibold text-[--color-void] transition-colors hover:bg-[--color-pulse-dim] disabled:opacity-50"
                  >
                    Add
                  </button>
                </div>
                <button
                  type="button"
                  onClick={() => setShowCustom(false)}
                  className="mt-2 text-xs text-[--color-dim] hover:text-[--color-ghost]"
                >
                  Cancel
                </button>
              </motion.form>
            ) : (
              <motion.button
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                onClick={() => setShowCustom(true)}
                className="mt-4 flex w-full items-center justify-center gap-2 rounded-lg border border-dashed border-[--color-steel] py-3 text-sm text-[--color-dim] transition-colors hover:border-[--color-ghost] hover:text-[--color-ghost]"
              >
                <Plus className="h-4 w-4" />
                Add custom node
              </motion.button>
            )}
          </AnimatePresence>

          {/* Error message */}
          <AnimatePresence>
            {error && (
              <motion.div
                initial={{ opacity: 0, y: -10 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -10 }}
                className="mt-4 rounded-lg bg-[--color-danger]/10 px-4 py-3 text-sm text-[--color-danger]"
              >
                {error}
              </motion.div>
            )}
          </AnimatePresence>
        </motion.div>

        {/* Help text */}
        <motion.p
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ delay: 0.6 }}
          className="mt-6 text-center text-xs text-[--color-dim]"
        >
          Start a node with{' '}
          <code className="rounded bg-[--color-slate] px-1.5 py-0.5 font-mono text-[--color-ghost]">
            cadence run
          </code>
        </motion.p>
      </motion.div>
    </div>
  )
}
