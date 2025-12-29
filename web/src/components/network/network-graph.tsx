import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  useNodesState,
  useEdgesState,
  type Node,
  type Edge,
  type NodeTypes,
  ConnectionLineType,
  MarkerType,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { PeerNode, type PeerNodeData } from './peer-node'

export interface NetworkPeer {
  id: string
  nodeId: string
  isValidator: boolean
  isSelf: boolean
  status: 'online' | 'syncing' | 'offline'
  latency?: number
  blockHeight?: number
  version?: string
  connectedTo: string[] // IDs of connected peers
}

interface NetworkGraphProps {
  peers: NetworkPeer[]
  onPeerSelect?: (peer: NetworkPeer | null) => void
}

const nodeTypes: NodeTypes = {
  peer: PeerNode,
}

// Simple force simulation for layout
function forceSimulation(
  nodes: Node<PeerNodeData>[],
  edges: Edge[],
  width: number,
  height: number,
  iterations: number = 100
): Node<PeerNodeData>[] {
  const positions = new Map<string, { x: number; y: number; vx: number; vy: number }>()

  // Initialize positions in a circle
  const centerX = width / 2
  const centerY = height / 2
  const radius = Math.min(width, height) * 0.35

  nodes.forEach((node, i) => {
    const angle = (2 * Math.PI * i) / nodes.length
    positions.set(node.id, {
      x: centerX + radius * Math.cos(angle),
      y: centerY + radius * Math.sin(angle),
      vx: 0,
      vy: 0,
    })
  })

  // Run simulation
  const repulsion = 8000
  const attraction = 0.05
  const damping = 0.85
  const minDistance = 150

  for (let iter = 0; iter < iterations; iter++) {
    const alpha = 1 - iter / iterations

    // Repulsion between all nodes
    nodes.forEach((nodeA) => {
      const posA = positions.get(nodeA.id)!
      nodes.forEach((nodeB) => {
        if (nodeA.id === nodeB.id) return
        const posB = positions.get(nodeB.id)!

        const dx = posA.x - posB.x
        const dy = posA.y - posB.y
        const distance = Math.sqrt(dx * dx + dy * dy) || 1
        const force = (repulsion * alpha) / (distance * distance)

        posA.vx += (dx / distance) * force
        posA.vy += (dy / distance) * force
      })
    })

    // Attraction along edges
    edges.forEach((edge) => {
      const posA = positions.get(edge.source)
      const posB = positions.get(edge.target)
      if (!posA || !posB) return

      const dx = posB.x - posA.x
      const dy = posB.y - posA.y
      const distance = Math.sqrt(dx * dx + dy * dy) || 1

      // Only attract if beyond minimum distance
      if (distance > minDistance) {
        const force = (distance - minDistance) * attraction * alpha

        posA.vx += (dx / distance) * force
        posA.vy += (dy / distance) * force
        posB.vx -= (dx / distance) * force
        posB.vy -= (dy / distance) * force
      }
    })

    // Center gravity
    nodes.forEach((node) => {
      const pos = positions.get(node.id)!
      const dx = centerX - pos.x
      const dy = centerY - pos.y
      pos.vx += dx * 0.01 * alpha
      pos.vy += dy * 0.01 * alpha
    })

    // Apply velocities with damping
    positions.forEach((pos) => {
      pos.x += pos.vx
      pos.y += pos.vy
      pos.vx *= damping
      pos.vy *= damping

      // Keep in bounds
      pos.x = Math.max(100, Math.min(width - 100, pos.x))
      pos.y = Math.max(100, Math.min(height - 100, pos.y))
    })
  }

  return nodes.map((node) => {
    const pos = positions.get(node.id)!
    return {
      ...node,
      position: { x: pos.x - 90, y: pos.y - 40 }, // Offset for node size
    }
  })
}

function truncateNodeId(nodeId: string): string {
  if (nodeId.length <= 12) return nodeId
  return `${nodeId.slice(0, 6)}...${nodeId.slice(-4)}`
}

export function NetworkGraph({ peers, onPeerSelect }: NetworkGraphProps) {
  const [dimensions, setDimensions] = useState({ width: 800, height: 600 })

  // Convert peers to nodes and edges
  const { initialNodes, initialEdges } = useMemo(() => {
    const nodes: Node<PeerNodeData>[] = peers.map((peer) => ({
      id: peer.id,
      type: 'peer',
      position: { x: 0, y: 0 },
      data: {
        label: truncateNodeId(peer.nodeId),
        nodeId: peer.nodeId,
        isValidator: peer.isValidator,
        isSelf: peer.isSelf,
        status: peer.status,
        latency: peer.latency,
        blockHeight: peer.blockHeight,
        version: peer.version,
      },
    }))

    const edgeSet = new Set<string>()
    const edges: Edge[] = []

    peers.forEach((peer) => {
      peer.connectedTo.forEach((targetId) => {
        // Create unique edge ID (sorted to avoid duplicates)
        const edgeId = [peer.id, targetId].sort().join('-')
        if (!edgeSet.has(edgeId)) {
          edgeSet.add(edgeId)
          edges.push({
            id: edgeId,
            source: peer.id,
            target: targetId,
            type: 'smoothstep',
            animated: peer.isSelf || peers.find((p) => p.id === targetId)?.isSelf,
            style: {
              stroke: 'var(--color-steel)',
              strokeWidth: 2,
            },
            markerEnd: {
              type: MarkerType.ArrowClosed,
              color: 'var(--color-steel)',
              width: 15,
              height: 15,
            },
          })
        }
      })
    })

    // Apply force layout
    const layoutedNodes = forceSimulation(nodes, edges, dimensions.width, dimensions.height)

    return { initialNodes: layoutedNodes, initialEdges: edges }
  }, [peers, dimensions])

  const [nodes, setNodes, onNodesChange] = useNodesState(initialNodes)
  const [edges, setEdges, onEdgesChange] = useEdgesState(initialEdges)

  // Update when peers change
  useEffect(() => {
    setNodes(initialNodes)
    setEdges(initialEdges)
  }, [initialNodes, initialEdges, setNodes, setEdges])

  const onNodeClick = useCallback(
    (_: React.MouseEvent, node: Node) => {
      const peer = peers.find((p) => p.id === node.id)
      onPeerSelect?.(peer || null)
    },
    [peers, onPeerSelect]
  )

  const onPaneClick = useCallback(() => {
    onPeerSelect?.(null)
  }, [onPeerSelect])

  return (
    <div className="h-full w-full" ref={(el) => {
      if (el) {
        const rect = el.getBoundingClientRect()
        if (rect.width !== dimensions.width || rect.height !== dimensions.height) {
          setDimensions({ width: rect.width, height: rect.height })
        }
      }
    }}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onNodeClick={onNodeClick}
        onPaneClick={onPaneClick}
        nodeTypes={nodeTypes}
        connectionLineType={ConnectionLineType.SmoothStep}
        fitView
        fitViewOptions={{ padding: 0.2 }}
        minZoom={0.3}
        maxZoom={1.5}
        proOptions={{ hideAttribution: true }}
        className="bg-transparent"
      >
        <Background
          color="var(--color-steel)"
          gap={50}
          size={1}
          style={{ opacity: 0.3 }}
        />
        <Controls
          className="!bg-[--color-slate] !border-[--color-steel] !rounded-lg !shadow-lg [&>button]:!bg-[--color-slate] [&>button]:!border-[--color-steel] [&>button]:!text-[--color-ghost] [&>button:hover]:!bg-[--color-abyss]"
        />
        <MiniMap
          nodeColor={(node) => {
            const data = node.data as PeerNodeData | undefined
            if (data?.isSelf) return 'var(--color-pulse)'
            if (data?.isValidator) return 'var(--color-purple)'
            return 'var(--color-steel)'
          }}
          maskColor="rgba(0, 0, 0, 0.8)"
          className="!bg-[--color-abyss] !border-[--color-steel] !rounded-lg"
        />
      </ReactFlow>
    </div>
  )
}
