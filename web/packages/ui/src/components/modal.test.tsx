/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'
import { ModalOverlay } from './modal'

function setup(overrides: Partial<React.ComponentProps<typeof ModalOverlay>> = {}) {
  const onDismiss = vi.fn()
  render(
    <ModalOverlay onDismiss={onDismiss} {...overrides}>
      <div data-testid="panel">
        <button data-testid="inner-button">Inside</button>
      </div>
    </ModalOverlay>,
  )
  return { onDismiss }
}

function getBackdrop(): HTMLElement {
  return screen.getByRole('dialog')
}

/** A real user click fires mousedown before click; mirror that here. */
function clickBackdrop() {
  const backdrop = getBackdrop()
  fireEvent.mouseDown(backdrop)
  fireEvent.click(backdrop)
}

describe('ModalOverlay dismissal policy (#655)', () => {
  beforeEach(() => cleanup())

  it('renders a dialog with aria-modal', () => {
    setup({ ariaLabelledBy: 'some-title' })
    const backdrop = getBackdrop()
    expect(backdrop.getAttribute('aria-modal')).toBe('true')
    expect(backdrop.getAttribute('aria-labelledby')).toBe('some-title')
  })

  it('dismisses on backdrop click', () => {
    const { onDismiss } = setup()
    clickBackdrop()
    expect(onDismiss).toHaveBeenCalledTimes(1)
  })

  it('does NOT dismiss on a click inside the panel', () => {
    const { onDismiss } = setup()
    const inner = screen.getByTestId('inner-button')
    fireEvent.mouseDown(inner)
    fireEvent.click(inner)
    expect(onDismiss).not.toHaveBeenCalled()
  })

  it('does NOT dismiss when a drag starts inside the panel and releases over the backdrop', () => {
    // The classic accidental-close: select text in an input, release the mouse
    // over the backdrop. The browser fires the click on the common ancestor
    // (the backdrop), but the mousedown target was inside the panel.
    const { onDismiss } = setup()
    fireEvent.mouseDown(screen.getByTestId('inner-button'))
    fireEvent.click(getBackdrop())
    expect(onDismiss).not.toHaveBeenCalled()
  })

  it('dismisses on Escape', () => {
    const { onDismiss } = setup()
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(onDismiss).toHaveBeenCalledTimes(1)
  })

  it('ignores other keys', () => {
    const { onDismiss } = setup()
    fireEvent.keyDown(document, { key: 'Enter' })
    fireEvent.keyDown(document, { key: 'a' })
    expect(onDismiss).not.toHaveBeenCalled()
  })

  it('suppresses backdrop click and Escape when dismissable={false}', () => {
    const { onDismiss } = setup({ dismissable: false })
    clickBackdrop()
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(onDismiss).not.toHaveBeenCalled()
  })

  it('removes the Escape listener on unmount', () => {
    const onDismiss = vi.fn()
    const { unmount } = render(
      <ModalOverlay onDismiss={onDismiss}>
        <div />
      </ModalOverlay>,
    )
    unmount()
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(onDismiss).not.toHaveBeenCalled()
  })
})
