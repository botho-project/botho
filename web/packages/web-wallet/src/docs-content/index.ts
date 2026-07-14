/**
 * Docs content registry (issue #778, i18n phase 3).
 *
 * The docs page body (~1500 lines of markdown across 9 sections) is far too
 * large to extract key-by-key into a JSON `t()` namespace the way phases 1–2
 * handled the landing/wallet/pay/claim/contacts copy. Instead each section's
 * markdown lives in its own per-locale `.md` file under `docs-content/<locale>/`,
 * imported here with Vite's `?raw` suffix so the content is statically bundled
 * (consistent with `i18n.ts`'s deliberate no-lazy-load / no-flash-of-English
 * decision) and stays plain markdown — easy to diff, review and translate.
 *
 * `id`, `icon` and ordering are locale-INVARIANT and live in `SECTION_META`
 * below. The `id` slug is URL-addressable (`/docs#<id>`, the legacy
 * `/docs/<id>` redirect, and internal `[...](#<id>)` cross-links all key off it)
 * and MUST NOT be translated — see `docs-deep-link.test.tsx` (#656 guard).
 * `icon` is a non-serializable `lucide-react` component reference, so it cannot
 * live in a JSON/markdown resource. Only the section TITLE (a short nav label /
 * H1, stored in the `docs` i18n namespace) and the markdown BODY (these files)
 * are locale-dependent.
 *
 * Content selection falls back to English when a locale is missing a section
 * file, mirroring `i18n.ts`'s `fallbackLng: DEFAULT_LOCALE` behavior.
 */
import type { LucideIcon } from 'lucide-react'
import {
  Book,
  Shield,
  Tag,
  Scale,
  Zap,
  Terminal,
  Code,
  Globe,
  Coins,
} from 'lucide-react'

import { DEFAULT_LOCALE, type SupportedLocale } from '../lib/i18n'

// --- English markdown bodies -------------------------------------------------
import enGettingStarted from './en/getting-started.md?raw'
import enPrivacy from './en/privacy.md?raw'
import enClusterTags from './en/cluster-tags.md?raw'
import enPrivacyProgressivity from './en/privacy-progressivity.md?raw'
import enConsensus from './en/consensus.md?raw'
import enRunningNode from './en/running-node.md?raw'
import enApi from './en/api.md?raw'
import enNetwork from './en/network.md?raw'
import enTokenomics from './en/tokenomics.md?raw'

// --- Spanish markdown bodies -------------------------------------------------
import esGettingStarted from './es/getting-started.md?raw'
import esPrivacy from './es/privacy.md?raw'
import esClusterTags from './es/cluster-tags.md?raw'
import esPrivacyProgressivity from './es/privacy-progressivity.md?raw'
import esConsensus from './es/consensus.md?raw'
import esRunningNode from './es/running-node.md?raw'
import esApi from './es/api.md?raw'
import esNetwork from './es/network.md?raw'
import esTokenomics from './es/tokenomics.md?raw'

// --- Simplified Chinese markdown bodies --------------------------------------
import zhGettingStarted from './zh/getting-started.md?raw'
import zhPrivacy from './zh/privacy.md?raw'
import zhClusterTags from './zh/cluster-tags.md?raw'
import zhPrivacyProgressivity from './zh/privacy-progressivity.md?raw'
import zhConsensus from './zh/consensus.md?raw'
import zhRunningNode from './zh/running-node.md?raw'
import zhApi from './zh/api.md?raw'
import zhNetwork from './zh/network.md?raw'
import zhTokenomics from './zh/tokenomics.md?raw'

/** A documentation section id — a stable, English, URL-addressable slug. */
export type DocSectionId =
  | 'getting-started'
  | 'privacy'
  | 'cluster-tags'
  | 'privacy-progressivity'
  | 'consensus'
  | 'running-node'
  | 'api'
  | 'network'
  | 'tokenomics'

export interface DocSectionMeta {
  /** Stable URL slug — NEVER localized (routing + cross-links depend on it). */
  id: DocSectionId
  /** Non-serializable lucide-react icon reference — locale-invariant. */
  icon: LucideIcon
}

/**
 * Locale-invariant section metadata, in render order. Adding/removing a section
 * here must be mirrored by matching `<locale>/<id>.md` files and a `sections.<id>`
 * title entry in every `locales/<locale>/docs.json`.
 */
export const SECTION_META: readonly DocSectionMeta[] = [
  { id: 'getting-started', icon: Book },
  { id: 'privacy', icon: Shield },
  { id: 'cluster-tags', icon: Tag },
  { id: 'privacy-progressivity', icon: Scale },
  { id: 'consensus', icon: Zap },
  { id: 'running-node', icon: Terminal },
  { id: 'api', icon: Code },
  { id: 'network', icon: Globe },
  { id: 'tokenomics', icon: Coins },
]

type ContentMap = Record<DocSectionId, string>

const CONTENT_BY_LOCALE: Record<SupportedLocale, ContentMap> = {
  en: {
    'getting-started': enGettingStarted,
    privacy: enPrivacy,
    'cluster-tags': enClusterTags,
    'privacy-progressivity': enPrivacyProgressivity,
    consensus: enConsensus,
    'running-node': enRunningNode,
    api: enApi,
    network: enNetwork,
    tokenomics: enTokenomics,
  },
  es: {
    'getting-started': esGettingStarted,
    privacy: esPrivacy,
    'cluster-tags': esClusterTags,
    'privacy-progressivity': esPrivacyProgressivity,
    consensus: esConsensus,
    'running-node': esRunningNode,
    api: esApi,
    network: esNetwork,
    tokenomics: esTokenomics,
  },
  zh: {
    'getting-started': zhGettingStarted,
    privacy: zhPrivacy,
    'cluster-tags': zhClusterTags,
    'privacy-progressivity': zhPrivacyProgressivity,
    consensus: zhConsensus,
    'running-node': zhRunningNode,
    api: zhApi,
    network: zhNetwork,
    tokenomics: zhTokenomics,
  },
}

/**
 * Return the markdown body for `id` in `locale`, falling back to the default
 * locale's content when the requested locale is missing that section (mirrors
 * i18next's `fallbackLng`). Falls back further to an empty string only if the
 * id is somehow unknown, so a bad lookup renders blank rather than crashing.
 */
export function getSectionContent(id: DocSectionId, locale: SupportedLocale): string {
  const localized = CONTENT_BY_LOCALE[locale]?.[id]
  if (localized != null && localized.trim() !== '') {
    return localized
  }
  return CONTENT_BY_LOCALE[DEFAULT_LOCALE][id] ?? ''
}
