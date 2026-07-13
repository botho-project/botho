/**
 * @vitest-environment jsdom
 *
 * Locale-rendering coverage for the node checkout / success / status pages
 * (issue #793, i18n phase 5a). Asserts page-owned copy renders in the active
 * locale under both the default and `/es`-prefixed paths, and that the
 * `node-checkout` region catalog (`labelKey` → `t()`) and the status-page state
 * badges / health summary switch language too.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import type { NodeStatus } from '../lib/node-status'

const fetchNodeStatusMock = vi.fn()
vi.mock('../lib/node-status', async () => {
  const actual = await vi.importActual<typeof import('../lib/node-status')>('../lib/node-status')
  return {
    ...actual,
    fetchNodeStatus: (token: string) => fetchNodeStatusMock(token),
  }
})

// Imported AFTER the mocks are registered.
import { NodePage, NodeSuccessPage, NodeStatusPage } from './node'
import i18n from '../lib/i18n'

const RUNNING_STATUS: NodeStatus = {
  nodeId: 'node-1',
  rpcUrl: 'https://rpc.example.test',
  state: 'running',
  region: 'us-west-2',
  health: { status: 'online', chainHeight: 42, synced: true },
  walletDeepLink: 'https://wallet.example.test/?rpc=...',
}

function renderAt(path: string, element: React.ReactElement) {
  return render(<MemoryRouter initialEntries={[path]}>{element}</MemoryRouter>)
}

describe('node pages i18n', () => {
  beforeEach(() => {
    fetchNodeStatusMock.mockReset()
    fetchNodeStatusMock.mockResolvedValue(RUNNING_STATUS)
    // The status page reads its token from window.location.search.
    window.history.replaceState({}, '', '/node/status?token=abc')
    return i18n.changeLanguage('en')
  })

  afterEach(() => cleanup())

  it('renders English checkout copy by default', () => {
    renderAt('/node', <NodePage />)
    expect(
      screen.getByRole('heading', { name: 'Host a Node for Your Community' }),
    ).toBeTruthy()
    expect(screen.getByRole('button', { name: /Subscribe/i })).toBeTruthy()
    // Region catalog resolves through labelKey → t().
    expect(screen.getByText('US West (Oregon) — us-west-2')).toBeTruthy()
  })

  it('renders Spanish checkout copy when the active locale is es', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/node', <NodePage />)
    expect(screen.getByRole('heading', { name: 'Aloja un nodo para tu comunidad' })).toBeTruthy()
    expect(screen.getByRole('button', { name: /Suscríbete/i })).toBeTruthy()
    expect(screen.getByText('Oeste de EE. UU. (Oregón) — us-west-2')).toBeTruthy()
    // English source string must NOT leak through untranslated.
    expect(
      screen.queryByRole('heading', { name: 'Host a Node for Your Community' }),
    ).toBeNull()
  })

  it('renders the success page in English by default', () => {
    renderAt('/node/success', <NodeSuccessPage />)
    expect(screen.getByRole('heading', { name: 'Subscription started' })).toBeTruthy()
  })

  it('renders the success page in Spanish under /es', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/node/success', <NodeSuccessPage />)
    expect(screen.getByRole('heading', { name: 'Suscripción iniciada' })).toBeTruthy()
    expect(screen.queryByRole('heading', { name: 'Subscription started' })).toBeNull()
  })

  it('renders the status page state badge in English by default', async () => {
    renderAt('/node/status', <NodeStatusPage />)
    expect(await screen.findByText('Running')).toBeTruthy()
  })

  it('renders the status page state badge in Spanish under /es', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/node/status', <NodeStatusPage />)
    expect(await screen.findByText('En ejecución')).toBeTruthy()
    await waitFor(() => expect(screen.queryByText('Running')).toBeNull())
  })
})
