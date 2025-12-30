import { useState, useCallback } from 'react'

export interface UseCopyToClipboardOptions {
  /** Duration to show "copied" state in ms (default: 2000) */
  timeout?: number
  /** Callback when copy succeeds */
  onSuccess?: () => void
  /** Callback when copy fails */
  onError?: (error: Error) => void
}

export interface UseCopyToClipboardReturn {
  /** Whether the text was recently copied */
  copied: boolean
  /** Copy text to clipboard */
  copy: (text: string) => Promise<boolean>
  /** Reset the copied state */
  reset: () => void
}

/**
 * Hook for copying text to clipboard with feedback state.
 *
 * @example
 * ```tsx
 * const { copied, copy } = useCopyToClipboard()
 *
 * <button onClick={() => copy(address)}>
 *   {copied ? <Check /> : <Copy />}
 * </button>
 * ```
 */
export function useCopyToClipboard(
  options: UseCopyToClipboardOptions = {}
): UseCopyToClipboardReturn {
  const { timeout = 2000, onSuccess, onError } = options
  const [copied, setCopied] = useState(false)

  const reset = useCallback(() => {
    setCopied(false)
  }, [])

  const copy = useCallback(
    async (text: string): Promise<boolean> => {
      if (!navigator?.clipboard) {
        const error = new Error('Clipboard API not available')
        onError?.(error)
        return false
      }

      try {
        await navigator.clipboard.writeText(text)
        setCopied(true)
        onSuccess?.()

        // Auto-reset after timeout
        setTimeout(() => {
          setCopied(false)
        }, timeout)

        return true
      } catch (err) {
        const error = err instanceof Error ? err : new Error('Failed to copy')
        onError?.(error)
        setCopied(false)
        return false
      }
    },
    [timeout, onSuccess, onError]
  )

  return { copied, copy, reset }
}
