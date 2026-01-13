import { useState } from 'react'
import { Logo, Card, CardHeader, CardTitle, CardContent, Button, Input } from '@botho/ui'
import { motion } from 'motion/react'
import { useConnection } from '../contexts/connection'
import {
  AlertTriangle,
  Loader2,
  Radio,
  Server,
  RefreshCw,
  Plus,
  AlertCircle,
} from 'lucide-react'
import type { NodeInfo } from '@botho/core'

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

  const handleAddCustom = async () => {
    await addCustomNode(customHost, parseInt(customPort, 10))
    setShowCustom(false)
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-[--color-void]">
      {/* Testnet banner */}
      <div className="fixed top-0 left-0 right-0 z-[60] flex h-7 items-center justify-center gap-2 bg-[--color-warning] text-[--color-void]">
        <AlertTriangle className="h-3.5 w-3.5" />
        <span className="text-xs font-bold uppercase tracking-wider">
          Testnet — Coins have no real value
        </span>
      </div>

      {/* Background effects */}
      <div className="fixed inset-0 grid-pattern opacity-30" />
      <div className="fixed top-1/4 left-1/2 -translate-x-1/2 h-[400px] w-[600px] bg-[--color-pulse]/10 blur-[120px] rounded-full" />

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        className="relative z-10 w-full max-w-md px-4"
      >
        <div className="text-center mb-8">
          <Logo size="lg" className="justify-center" />
          <p className="mt-4 text-[--color-ghost]">
            Connect to a local Botho node to get started
          </p>
        </div>

        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Radio className="h-4 w-4 text-[--color-pulse]" />
              <CardTitle>Available Nodes</CardTitle>
            </div>
            <Button
              variant="ghost"
              size="icon"
              onClick={scanForNodes}
              disabled={isScanning}
            >
              <RefreshCw className={`h-4 w-4 ${isScanning ? 'animate-spin' : ''}`} />
            </Button>
          </CardHeader>

          <CardContent className="space-y-4">
            {isScanning ? (
              <div className="flex items-center justify-center py-8">
                <Loader2 className="h-8 w-8 animate-spin text-[--color-pulse]" />
                <span className="ml-3 text-[--color-ghost]">Scanning for nodes...</span>
              </div>
            ) : discoveredNodes.length === 0 ? (
              <div className="text-center py-8">
                <Server className="mx-auto h-12 w-12 text-[--color-dim]" />
                <p className="mt-4 text-[--color-ghost]">No nodes found</p>
                <p className="mt-1 text-sm text-[--color-dim]">
                  Make sure a Botho node is running locally
                </p>
              </div>
            ) : (
              <div className="space-y-2">
                {discoveredNodes.map((node) => (
                  <NodeCard
                    key={node.id}
                    node={node}
                    onConnect={() => connectToNode(node)}
                  />
                ))}
              </div>
            )}

            {error && (
              <div className="flex items-center gap-2 rounded-lg bg-[--color-danger]/10 p-3 text-sm text-[--color-danger]">
                <AlertCircle className="h-4 w-4 shrink-0" />
                {error}
              </div>
            )}

            {/* Add custom node */}
            {showCustom ? (
              <div className="space-y-3 border-t border-[--color-steel] pt-4">
                <div className="flex gap-2">
                  <Input
                    value={customHost}
                    onChange={(e) => setCustomHost(e.target.value)}
                    placeholder="Host"
                    className="flex-1"
                  />
                  <Input
                    value={customPort}
                    onChange={(e) => setCustomPort(e.target.value)}
                    placeholder="Port"
                    className="w-24"
                  />
                </div>
                <div className="flex gap-2">
                  <Button
                    variant="secondary"
                    className="flex-1"
                    onClick={() => setShowCustom(false)}
                  >
                    Cancel
                  </Button>
                  <Button className="flex-1" onClick={handleAddCustom}>
                    Add Node
                  </Button>
                </div>
              </div>
            ) : (
              <button
                onClick={() => setShowCustom(true)}
                className="flex w-full items-center justify-center gap-2 rounded-lg border border-dashed border-[--color-steel] py-3 text-sm text-[--color-ghost] transition-colors hover:border-[--color-pulse-dim] hover:text-[--color-light]"
              >
                <Plus className="h-4 w-4" />
                Add custom node
              </button>
            )}
          </CardContent>
        </Card>

        <p className="mt-6 text-center text-xs text-[--color-dim]">
          Need help?{' '}
          <a
            href="https://botho.io/docs/getting-started"
            target="_blank"
            rel="noopener noreferrer"
            className="text-[--color-pulse] hover:underline"
          >
            Getting Started Guide
          </a>
        </p>
      </motion.div>
    </div>
  )
}

function NodeCard({ node, onConnect }: { node: NodeInfo; onConnect: () => void }) {
  return (
    <motion.div
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0 }}
      className="flex items-center justify-between rounded-lg border border-[--color-steel] bg-[--color-slate] p-4 transition-colors hover:border-[--color-pulse-dim]"
    >
      <div className="flex items-center gap-3">
        <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-pulse]/10">
          <Server className="h-5 w-5 text-[--color-pulse]" />
        </div>
        <div>
          <p className="font-mono text-sm text-[--color-light]">
            {node.host}:{node.port}
          </p>
          <div className="flex items-center gap-2 text-xs text-[--color-dim]">
            <span className="flex items-center gap-1">
              <span className="h-1.5 w-1.5 rounded-full bg-[--color-success]" />
              {node.latency}ms
            </span>
            {node.blockHeight && (
              <>
                <span>•</span>
                <span>Block {node.blockHeight.toLocaleString()}</span>
              </>
            )}
            {node.version && (
              <>
                <span>•</span>
                <span>{node.version}</span>
              </>
            )}
          </div>
        </div>
      </div>
      <Button size="sm" onClick={onConnect}>
        Connect
      </Button>
    </motion.div>
  )
}
