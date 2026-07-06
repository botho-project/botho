/**
 * @vitest-environment jsdom
 *
 * Docs path deep links (#656).
 *
 * The docs page selects its active section from the URL hash (`/docs#<id>`),
 * but the router also registers a `/docs/*` catchall. Before #656 any
 * path-style deep link (`/docs/cluster-tags`) silently rendered the default
 * "Getting Started" section. The guarantees under test:
 *
 * - `/docs/<known-id>` redirects (history `replace`) to the canonical
 *   `/docs#<known-id>` form and renders that section.
 * - `/docs/<unknown>` renders Getting Started WITH a visible "not found"
 *   hint naming the requested segment (no redirect — the bad URL stays
 *   visible for debugging).
 * - Plain `/docs`, `/docs/`, and hash links behave exactly as before.
 * - When both a path segment and a hash are present, the hash wins.
 */
import { describe, it, expect, afterEach } from 'vitest'
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react'
import { MemoryRouter, Routes, Route, useLocation, useNavigate } from 'react-router-dom'
import { DocsPage } from './docs'

/** Every valid section id and its page heading, mirroring `sections` in docs.tsx. */
const SECTION_TITLES: Record<string, string> = {
  'getting-started': 'Getting Started',
  privacy: 'Privacy Features',
  'cluster-tags': 'Cluster Tags',
  'privacy-progressivity': 'Privacy + Progressivity',
  consensus: 'Consensus',
  'running-node': 'Running a Node',
  api: 'API Reference',
  network: 'Network',
  tokenomics: 'Tokenomics',
}

const NOT_FOUND_HINT = /not found — showing Getting Started/

/** Exposes the router location and a Back button for history assertions. */
function LocationProbe() {
  const location = useLocation()
  const navigate = useNavigate()
  return (
    <div>
      <span data-testid="location">{location.pathname + location.hash}</span>
      <button onClick={() => navigate(-1)}>go back</button>
    </div>
  )
}

/** Renders DocsPage with the same `/docs` + `/docs/*` route pair as App.tsx. */
function renderDocs(initialEntries: string[], initialIndex?: number) {
  return render(
    <MemoryRouter initialEntries={initialEntries} initialIndex={initialIndex}>
      <LocationProbe />
      <Routes>
        <Route path="/docs" element={<DocsPage />} />
        <Route path="/docs/*" element={<DocsPage />} />
        <Route path="*" element={<div data-testid="other-page">other page</div>} />
      </Routes>
    </MemoryRouter>,
  )
}

function activeHeading(): string | null {
  return screen.getByRole('heading', { level: 1 }).textContent
}

afterEach(cleanup)

describe('docs path deep links redirect to the canonical hash form (#656)', () => {
  it('/docs/cluster-tags lands on /docs#cluster-tags and renders Cluster Tags', async () => {
    renderDocs(['/docs/cluster-tags'])
    await waitFor(() =>
      expect(screen.getByTestId('location').textContent).toBe('/docs#cluster-tags'),
    )
    expect(activeHeading()).toBe('Cluster Tags')
    expect(screen.queryByText(NOT_FOUND_HINT)).toBeNull()
  })

  it.each(Object.entries(SECTION_TITLES))(
    '/docs/%s resolves via the path form',
    async (id, title) => {
      renderDocs([`/docs/${id}`])
      await waitFor(() => expect(screen.getByTestId('location').textContent).toBe(`/docs#${id}`))
      expect(activeHeading()).toBe(title)
      expect(screen.queryByText(NOT_FOUND_HINT)).toBeNull()
    },
  )

  it('matches section ids case-insensitively (/docs/Cluster-Tags)', async () => {
    renderDocs(['/docs/Cluster-Tags'])
    await waitFor(() =>
      expect(screen.getByTestId('location').textContent).toBe('/docs#cluster-tags'),
    )
    expect(activeHeading()).toBe('Cluster Tags')
  })

  it('redirect uses history replace — Back skips the path form (no loop)', async () => {
    renderDocs(['/home', '/docs/consensus'], 1)
    await waitFor(() => expect(screen.getByTestId('location').textContent).toBe('/docs#consensus'))

    fireEvent.click(screen.getByText('go back'))

    // With `replace: true` the path entry is gone; Back exits docs entirely
    // instead of bouncing back through /docs/consensus.
    await waitFor(() => expect(screen.getByTestId('location').textContent).toBe('/home'))
    expect(screen.getByTestId('other-page')).toBeTruthy()
  })
})

describe('unknown docs path segments show a not-found hint (#656)', () => {
  it('/docs/protocol renders Getting Started with a hint naming the segment', () => {
    renderDocs(['/docs/protocol'])
    expect(activeHeading()).toBe('Getting Started')
    const hint = screen.getByText(NOT_FOUND_HINT)
    expect(hint.textContent).toContain('protocol')
    // No redirect: the bad URL stays visible in the address bar.
    expect(screen.getByTestId('location').textContent).toBe('/docs/protocol')
  })

  it('nested segments (/docs/a/b) are treated as unknown, not a crash', () => {
    renderDocs(['/docs/a/b'])
    expect(activeHeading()).toBe('Getting Started')
    expect(screen.getByText(NOT_FOUND_HINT).textContent).toContain('a/b')
  })
})

describe('existing docs URL forms are unchanged (#656 regression guard)', () => {
  it('/docs renders Getting Started with no hint and no redirect', () => {
    renderDocs(['/docs'])
    expect(activeHeading()).toBe('Getting Started')
    expect(screen.queryByText(NOT_FOUND_HINT)).toBeNull()
    expect(screen.getByTestId('location').textContent).toBe('/docs')
  })

  it('/docs/ (trailing slash) renders Getting Started with no hint and no redirect', () => {
    renderDocs(['/docs/'])
    expect(activeHeading()).toBe('Getting Started')
    expect(screen.queryByText(NOT_FOUND_HINT)).toBeNull()
    expect(screen.getByTestId('location').textContent).toBe('/docs/')
  })

  it('/docs#consensus renders Consensus with no hint (hash path untouched)', () => {
    renderDocs(['/docs#consensus'])
    expect(activeHeading()).toBe('Consensus')
    expect(screen.queryByText(NOT_FOUND_HINT)).toBeNull()
    expect(screen.getByTestId('location').textContent).toBe('/docs#consensus')
  })

  it('hash wins when both a path segment and a hash are present', () => {
    renderDocs(['/docs/consensus#privacy'])
    expect(activeHeading()).toBe('Privacy Features')
    expect(screen.queryByText(NOT_FOUND_HINT)).toBeNull()
    // No redirect — the hash already picked the section.
    expect(screen.getByTestId('location').textContent).toBe('/docs/consensus#privacy')
  })
})
