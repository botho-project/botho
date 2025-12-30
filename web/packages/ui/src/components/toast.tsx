import { useEffect, useState } from 'react'
import { cn } from '../lib/utils'

interface ToastProps {
  message: string
  visible: boolean
  onHide?: () => void
  duration?: number
  icon?: React.ReactNode
}

export function Toast({ message, visible, onHide, duration = 2000, icon }: ToastProps) {
  const [show, setShow] = useState(false)

  useEffect(() => {
    if (visible) {
      setShow(true)
      const timer = setTimeout(() => {
        setShow(false)
        setTimeout(() => onHide?.(), 200) // Wait for fade out animation
      }, duration)
      return () => clearTimeout(timer)
    }
  }, [visible, duration, onHide])

  if (!visible && !show) return null

  return (
    <div className="fixed bottom-6 left-1/2 -translate-x-1/2 z-50 pointer-events-none">
      <div
        className={cn(
          'flex items-center gap-2 px-4 py-3 rounded-xl bg-[--color-slate] border border-[--color-steel] shadow-lg',
          'transition-all duration-200',
          show ? 'opacity-100 translate-y-0' : 'opacity-0 translate-y-2'
        )}
      >
        {icon}
        <span className="text-sm font-medium text-[--color-light]">{message}</span>
      </div>
    </div>
  )
}
