import { Button, Card, CardContent } from '@botho/ui'
import { ArrowRight, Coins, Lock, Repeat, ShieldCheck, Sparkles } from 'lucide-react'
import type { Translate } from '../types'

/** Step icons for the "how to bridge" flow, in render order. */
const STEP_KEYS = ['factor1', 'lock', 'mint', 'trade'] as const
const STEP_ICON = {
  factor1: Coins,
  lock: Lock,
  mint: Sparkles,
  trade: ArrowRight,
} as const

/** Peg-guarantee callouts. */
const GUARANTEE_KEYS = ['peg', 'exactlyOnce', 'factorOne'] as const

export interface ExportExplainerProps {
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  className?: string
}

/**
 * Guided BTH→wBTH export explainer (#1030, Tier 0).
 *
 * Explanatory copy + a "how it works" flow and the peg guarantees (1:1,
 * exactly-once, factor-1 / zero-demurrage). NO chain code — the integrated
 * flow lands in Tier 1 via the {@link ExportPanel} extension point below.
 */
export function ExportExplainer({ t, className }: ExportExplainerProps) {
  return (
    <section className={className}>
      <h2 className="font-display text-xl font-semibold text-[--color-light]">
        {t('export.heading')}
      </h2>
      <p className="mt-1 max-w-2xl text-sm text-[--color-dim]">{t('export.intro')}</p>

      {/* How it works — numbered flow. */}
      <ol className="mt-5 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        {STEP_KEYS.map((key, i) => {
          const Icon = STEP_ICON[key]
          return (
            <li key={key}>
              <Card className="h-full">
                <CardContent className="p-4">
                  <div className="flex items-center gap-2">
                    <span className="flex h-6 w-6 items-center justify-center rounded-full bg-[--color-pulse]/10 text-xs font-semibold text-[--color-pulse]">
                      {i + 1}
                    </span>
                    <Icon className="h-4 w-4 text-[--color-pulse]" />
                  </div>
                  <div className="mt-2.5 text-sm font-semibold text-[--color-light]">
                    {t(`export.steps.${key}.title`)}
                  </div>
                  <p className="mt-1 text-xs text-[--color-dim]">
                    {t(`export.steps.${key}.body`)}
                  </p>
                </CardContent>
              </Card>
            </li>
          )
        })}
      </ol>

      {/* Peg guarantees. */}
      <div className="mt-4 grid gap-4 sm:grid-cols-3">
        {GUARANTEE_KEYS.map((key) => (
          <div
            key={key}
            className="rounded-xl border border-[--color-steel] bg-[--color-abyss]/40 p-4"
          >
            <div className="flex items-center gap-1.5 text-sm font-medium text-[--color-soft]">
              <ShieldCheck className="h-4 w-4 text-emerald-400" />
              {t(`export.guarantees.${key}.title`)}
            </div>
            <p className="mt-1 text-xs text-[--color-dim]">
              {t(`export.guarantees.${key}.body`)}
            </p>
          </div>
        ))}
      </div>

      <ExportPanel t={t} className="mt-6" />
    </section>
  )
}

export interface ExportPanelProps {
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  className?: string
}

/**
 * Tier 1 EXTENSION POINT (#1029).
 *
 * For Tier 0 this is an inert scaffold: it explains that an integrated,
 * in-wallet export flow is coming and renders a disabled CTA. Tier 1 replaces
 * the disabled button + note with the real chain-aware export flow (amount
 * input, factor-1 check, lock + attestation, mint status) WITHOUT changing the
 * surrounding page — this is the single slot to wire it into.
 */
export function ExportPanel({ t, className }: ExportPanelProps) {
  return (
    <Card className={className}>
      <CardContent className="flex flex-col items-start gap-4 p-5 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-start gap-3">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-[--color-pulse]/10">
            <Repeat className="h-5 w-5 text-[--color-pulse]" />
          </div>
          <div>
            <div className="font-display text-base font-semibold text-[--color-light]">
              {t('export.cta.title')}
            </div>
            <p className="mt-1 max-w-xl text-sm text-[--color-dim]">{t('export.cta.body')}</p>
          </div>
        </div>
        <div className="flex flex-col items-start gap-1 sm:items-end">
          {/* Disabled until Tier 1 wires the integrated flow into this slot. */}
          <Button variant="secondary" disabled title={t('export.cta.disabledTitle')}>
            {t('export.cta.button')}
          </Button>
          <span className="text-[10px] uppercase tracking-wider text-[--color-dim]">
            {t('export.cta.badge')}
          </span>
        </div>
      </CardContent>
    </Card>
  )
}
