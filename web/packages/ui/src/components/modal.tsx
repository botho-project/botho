import { useEffect, useRef, type ReactNode } from 'react'
import { cn } from '../lib/utils'

export interface ModalOverlayProps {
  /**
   * Called when the user dismisses the modal via a backdrop click or the
   * Escape key. Route this through the modal's reset-and-close path (e.g. a
   * `handleClose` that clears form state) — the same handler the explicit X /
   * Cancel button uses — so every dismissal affordance behaves identically.
   */
  onDismiss: () => void
  /**
   * When false, backdrop clicks and Escape are ignored. Set this while an
   * async action is in flight (saving, sending, connecting) so the user cannot
   * orphan an in-progress operation by dismissing the modal.
   */
  dismissable?: boolean
  /**
   * Extra classes merged onto the backdrop element. Use this for the visual
   * treatment (dim colour, blur, flex centering) so packages can keep their
   * existing styling. The base always provides `fixed inset-0 z-50`.
   */
  className?: string
  /** Optional id of the element labelling the dialog (for `aria-labelledby`). */
  ariaLabelledBy?: string
  children: ReactNode
}

/**
 * Shared modal backdrop implementing one dismissal policy for every modal
 * (#655): clicking the backdrop or pressing Escape dismisses, clicks inside
 * the panel never do, and `dismissable={false}` suppresses both while an
 * async action is in flight.
 *
 * Backdrop clicks are validated against BOTH the `mousedown` and `click`
 * targets: a text-selection drag that starts inside an input and releases
 * over the backdrop fires its click on the common ancestor (the backdrop),
 * which must NOT dismiss the modal.
 */
export function ModalOverlay({
  onDismiss,
  dismissable = true,
  className,
  ariaLabelledBy,
  children,
}: ModalOverlayProps) {
  // Whether the most recent mousedown landed on the backdrop itself (not on
  // the panel or its contents).
  const mouseDownOnBackdrop = useRef(false)

  // Document-level Escape-to-dismiss while the modal is mounted. Modals in
  // this app never stack (callers close one before opening another), so a
  // per-modal listener needs no top-most bookkeeping.
  useEffect(() => {
    if (!dismissable) return
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !e.defaultPrevented) {
        onDismiss()
      }
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [dismissable, onDismiss])

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby={ariaLabelledBy}
      className={cn('fixed inset-0 z-50', className)}
      onMouseDown={(e) => {
        mouseDownOnBackdrop.current = e.target === e.currentTarget
      }}
      onClick={(e) => {
        if (dismissable && mouseDownOnBackdrop.current && e.target === e.currentTarget) {
          onDismiss()
        }
      }}
    >
      {children}
    </div>
  )
}
