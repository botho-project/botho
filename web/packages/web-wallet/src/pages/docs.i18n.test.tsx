/**
 * @vitest-environment jsdom
 *
 * Locale-rendering coverage for the docs page (issue #778, i18n phase 3).
 *
 * The docs body lives in per-locale markdown files (`docs-content/<locale>/*.md`)
 * imported with Vite `?raw`, while section titles live in the `docs` i18n
 * namespace. Section `id` slugs and the `#hash` scheme are locale-INVARIANT —
 * the English-only behavior is guarded separately by `docs-deep-link.test.tsx`.
 * Here we assert the SPANISH surface: `/es/docs` renders translated titles and
 * markdown, cross-locale content does not leak English, and a mid-session
 * language switch re-renders the active section in the new language.
 */
import { describe, it, expect, afterEach, beforeEach } from 'vitest'
import { render, screen, cleanup, waitFor } from '@testing-library/react'
import { MemoryRouter, Routes, Route } from 'react-router-dom'
import { DocsPage } from './docs'
import i18n from '../lib/i18n'

/** Mirrors the `/docs` + `/es/docs` + `/zh/docs` route pairs registered via LocaleRoutes. */
function renderDocs(initialEntries: string[]) {
  return render(
    <MemoryRouter initialEntries={initialEntries}>
      <Routes>
        <Route path="/docs" element={<DocsPage />} />
        <Route path="/docs/*" element={<DocsPage />} />
        <Route path="/es/docs" element={<DocsPage />} />
        <Route path="/es/docs/*" element={<DocsPage />} />
        <Route path="/zh/docs" element={<DocsPage />} />
        <Route path="/zh/docs/*" element={<DocsPage />} />
      </Routes>
    </MemoryRouter>,
  )
}

function activeHeading(): string | null {
  return screen.getByRole('heading', { level: 1 }).textContent
}

describe('DocsPage i18n (es)', () => {
  beforeEach(() => i18n.changeLanguage('en'))
  afterEach(() => {
    cleanup()
    return i18n.changeLanguage('en')
  })

  it('renders Spanish section titles and markdown under /es/docs', async () => {
    await i18n.changeLanguage('es')
    renderDocs(['/es/docs'])

    // Default section (getting-started) H1 uses the localized title.
    expect(activeHeading()).toBe('Primeros pasos')
    // Body markdown comes from docs-content/es/getting-started.md.
    expect(screen.getByText(/Botho es una criptomoneda centrada en la privacidad/)).toBeTruthy()
    // English source copy must NOT leak through untranslated.
    expect(screen.queryByText(/Botho is a privacy-focused cryptocurrency/)).toBeNull()
  })

  it('renders a Spanish deep-linked section (/es/docs#cluster-tags)', async () => {
    await i18n.changeLanguage('es')
    renderDocs(['/es/docs#cluster-tags'])

    expect(activeHeading()).toBe('Etiquetas de clúster')
    expect(screen.getByText(/Las etiquetas de clúster son el mecanismo novedoso de Botho/)).toBeTruthy()
  })

  it('nav labels are localized while the hrefs keep the invariant slug', async () => {
    await i18n.changeLanguage('es')
    renderDocs(['/es/docs'])

    // Localized nav label (desktop + mobile => at least one match).
    const consensusLinks = screen.getAllByRole('link', { name: 'Consenso' })
    expect(consensusLinks.length).toBeGreaterThan(0)
    // The href keeps the locale prefix + the English slug (locale-invariant id).
    expect(consensusLinks[0].getAttribute('href')).toBe('/es/docs#consensus')
  })

  it('re-renders the active section in the new language on a mid-session switch', async () => {
    await i18n.changeLanguage('es')
    renderDocs(['/es/docs#consensus'])
    expect(activeHeading()).toBe('Consenso')
    expect(
      screen.getByText(/SCP es un protocolo de acuerdo bizantino federado/),
    ).toBeTruthy()

    // Switching to English re-renders the same section under the English route.
    cleanup()
    await i18n.changeLanguage('en')
    renderDocs(['/docs#consensus'])
    await waitFor(() => expect(activeHeading()).toBe('Consensus'))
    expect(
      screen.getByText(/SCP is a federated Byzantine agreement protocol/),
    ).toBeTruthy()
  })

  it('renders the not-found hint in Spanish for an unknown /es/docs segment', async () => {
    await i18n.changeLanguage('es')
    renderDocs(['/es/docs/protocol'])
    // Getting Started title in Spanish, plus a Spanish not-found hint.
    expect(activeHeading()).toBe('Primeros pasos')
    const hint = screen.getByText(/no encontrada — mostrando Primeros pasos/)
    expect(hint.textContent).toContain('protocol')
  })
})

describe('DocsPage i18n (zh)', () => {
  beforeEach(() => i18n.changeLanguage('en'))
  afterEach(() => {
    cleanup()
    return i18n.changeLanguage('en')
  })

  it('renders Simplified Chinese section titles and markdown under /zh/docs', async () => {
    await i18n.changeLanguage('zh')
    renderDocs(['/zh/docs'])

    // Default section (getting-started) H1 uses the localized title.
    expect(activeHeading()).toBe('快速开始')
    // Body markdown comes from docs-content/zh/getting-started.md.
    expect(screen.getByText(/Botho 是一种以隐私为核心的加密货币/)).toBeTruthy()
    // English source copy must NOT leak through untranslated.
    expect(screen.queryByText(/Botho is a privacy-focused cryptocurrency/)).toBeNull()
  })

  it('renders a Simplified Chinese deep-linked section (/zh/docs#cluster-tags)', async () => {
    await i18n.changeLanguage('zh')
    renderDocs(['/zh/docs#cluster-tags'])

    expect(activeHeading()).toBe('集群标签')
    expect(screen.getByText(/用于追踪币的来源而不损害隐私的新颖机制/)).toBeTruthy()
  })

  it('nav labels are localized while the hrefs keep the invariant slug', async () => {
    await i18n.changeLanguage('zh')
    renderDocs(['/zh/docs'])

    const consensusLinks = screen.getAllByRole('link', { name: '共识' })
    expect(consensusLinks.length).toBeGreaterThan(0)
    // The href keeps the locale prefix + the English slug (locale-invariant id).
    expect(consensusLinks[0].getAttribute('href')).toBe('/zh/docs#consensus')
  })

  it('re-renders the active section in the new language on a mid-session switch', async () => {
    await i18n.changeLanguage('zh')
    renderDocs(['/zh/docs#consensus'])
    expect(activeHeading()).toBe('共识')
    expect(screen.getByText(/SCP 是一种联邦拜占庭协议/)).toBeTruthy()

    // Switching to English re-renders the same section under the English route.
    cleanup()
    await i18n.changeLanguage('en')
    renderDocs(['/docs#consensus'])
    await waitFor(() => expect(activeHeading()).toBe('Consensus'))
    expect(
      screen.getByText(/SCP is a federated Byzantine agreement protocol/),
    ).toBeTruthy()
  })

  it('renders the not-found hint in Simplified Chinese for an unknown /zh/docs segment', async () => {
    await i18n.changeLanguage('zh')
    renderDocs(['/zh/docs/protocol'])
    expect(activeHeading()).toBe('快速开始')
    const hint = screen.getByText(/未找到.*显示“快速开始”/)
    expect(hint.textContent).toContain('protocol')
  })
})
