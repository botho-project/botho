import { motion } from 'motion/react'
import { useEffect, useState } from 'react'

interface DataPoint {
  time: number
  value: number
}

export function PulseChart() {
  const [data, setData] = useState<DataPoint[]>([])

  useEffect(() => {
    // Generate initial data
    const initialData: DataPoint[] = []
    const now = Date.now()
    for (let i = 60; i >= 0; i--) {
      initialData.push({
        time: now - i * 1000,
        value: 50 + Math.random() * 50,
      })
    }
    setData(initialData)

    // Simulate live updates
    const interval = setInterval(() => {
      setData((prev) => {
        const newData = [...prev.slice(1)]
        newData.push({
          time: Date.now(),
          value: 50 + Math.random() * 50,
        })
        return newData
      })
    }, 1000)

    return () => clearInterval(interval)
  }, [])

  const maxValue = Math.max(...data.map((d) => d.value), 100)
  const minValue = Math.min(...data.map((d) => d.value), 0)
  const range = maxValue - minValue || 1

  const pathD = data
    .map((point, i) => {
      const x = (i / (data.length - 1)) * 100
      const y = 100 - ((point.value - minValue) / range) * 80 - 10
      return `${i === 0 ? 'M' : 'L'} ${x} ${y}`
    })
    .join(' ')

  const areaD = pathD + ` L 100 100 L 0 100 Z`

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.5, delay: 0.3 }}
      className="relative h-48 w-full overflow-hidden rounded-xl border border-[--color-steel] bg-[--color-abyss]/80"
    >
      {/* Header */}
      <div className="absolute left-4 top-4 z-10">
        <div className="flex items-center gap-2">
          <div className="h-2 w-2 rounded-full bg-[--color-pulse] pulse-indicator" />
          <span className="font-display text-xs font-semibold uppercase tracking-wider text-[--color-soft]">
            Network Pulse
          </span>
        </div>
        <p className="mt-1 font-display text-2xl font-bold text-[--color-light]">
          {data[data.length - 1]?.value.toFixed(0) || 0}
          <span className="ml-1 text-sm text-[--color-dim]">tx/s</span>
        </p>
      </div>

      {/* Chart */}
      <svg
        viewBox="0 0 100 100"
        preserveAspectRatio="none"
        className="absolute inset-0 h-full w-full"
      >
        <defs>
          <linearGradient id="pulse-gradient" x1="0%" y1="0%" x2="0%" y2="100%">
            <stop offset="0%" stopColor="var(--color-pulse)" stopOpacity="0.3" />
            <stop offset="100%" stopColor="var(--color-pulse)" stopOpacity="0" />
          </linearGradient>
          <filter id="glow">
            <feGaussianBlur stdDeviation="2" result="coloredBlur" />
            <feMerge>
              <feMergeNode in="coloredBlur" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        </defs>

        {/* Grid lines */}
        {[25, 50, 75].map((y) => (
          <line
            key={y}
            x1="0"
            y1={y}
            x2="100"
            y2={y}
            stroke="var(--color-steel)"
            strokeWidth="0.2"
            strokeDasharray="2 2"
          />
        ))}

        {/* Area fill */}
        <motion.path
          d={areaD}
          fill="url(#pulse-gradient)"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.5 }}
        />

        {/* Line */}
        <motion.path
          d={pathD}
          fill="none"
          stroke="var(--color-pulse)"
          strokeWidth="0.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          filter="url(#glow)"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 1 }}
        />

        {/* Current point */}
        {data.length > 0 && (
          <circle
            cx="100"
            cy={100 - ((data[data.length - 1].value - minValue) / range) * 80 - 10}
            r="1.5"
            fill="var(--color-pulse)"
            filter="url(#glow)"
          >
            <animate
              attributeName="r"
              values="1.5;2.5;1.5"
              dur="1s"
              repeatCount="indefinite"
            />
          </circle>
        )}
      </svg>

      {/* Time labels */}
      <div className="absolute bottom-2 left-4 right-4 flex justify-between text-[10px] text-[--color-dim]">
        <span>60s ago</span>
        <span>30s ago</span>
        <span>Now</span>
      </div>
    </motion.div>
  )
}
