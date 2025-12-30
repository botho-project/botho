import { Card, CardContent } from '@botho/ui'
import { motion, AnimatePresence } from 'motion/react'
import { AlertCircle } from 'lucide-react'
import { useExplorer } from '../context'

export interface ErrorMessageProps {
  /** Custom class name */
  className?: string
}

export function ErrorMessage({ className }: ErrorMessageProps) {
  const { error } = useExplorer()

  return (
    <AnimatePresence>
      {error && (
        <motion.div
          initial={{ opacity: 0, y: -10 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -10 }}
          className={className}
        >
          <Card className="border-[--color-danger]/50">
            <CardContent className="flex items-center gap-3 py-3">
              <AlertCircle className="h-5 w-5 text-[--color-danger]" />
              <p className="text-sm text-[--color-danger]">{error}</p>
            </CardContent>
          </Card>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
