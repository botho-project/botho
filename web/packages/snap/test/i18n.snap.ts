/**
 * i18n for the Snap dialogs (issue #1095).
 *
 * Two layers:
 *   1. PURE unit tests of the SES-safe message map + `t()` helper (no
 *      `installSnap`, no wasm) — per-locale lookups, `{placeholder}`
 *      substitution, `en` fallback for an unsupported locale, Intl-free
 *      pluralization, and a key-parity assertion so a missing/renamed
 *      translation fails CI instead of silently falling back.
 *   2. A SES-harness render test: with the MetaMask user preference set to
 *      `es`/`zh` (the harness honors `installSnap`'s `locale` option, which it
 *      surfaces through `snap_getPreferences` — exactly the signal the Snap reads
 *      at runtime), a dialog renders its localized heading; an unsupported locale
 *      falls back to the English heading.
 */

import { describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';

import {
  t,
  narrowLocale,
  confirmationsPhrase,
  SUPPORTED_LOCALES,
  DEFAULT_LOCALE,
  type MessageKey,
} from '../src/i18n';
import { en } from '../src/locales/en';
import { es } from '../src/locales/es';
import { zh } from '../src/locales/zh';

describe('i18n: message map + t() (pure, SES-free)', () => {
  it('returns the localized string per locale', () => {
    expect(t('receive.heading', 'en')).toBe('Receive BTH');
    expect(t('receive.heading', 'es')).toBe('Recibir BTH');
    expect(t('receive.heading', 'zh')).toBe('接收 BTH');

    expect(t('send.heading', 'es')).toBe('Confirmar envío');
    expect(t('send.recipient', 'zh')).toBe('收款人');
    expect(t('mnemonic.heading', 'es')).toBe('Frase de recuperación de Botho');
  });

  it('substitutes {placeholder} params without Intl', () => {
    expect(t('claim.hint', 'en', { amount: '1.5 BTH' })).toBe(
      'Link hint: 1.5 BTH (cosmetic — the scanned amount above is authoritative)',
    );
    expect(t('claim.hint', 'es', { amount: '1.5 BTH' })).toContain('1.5 BTH');

    // The history line composes a nested, already-localized confirmation phrase.
    expect(
      t('history.line', 'en', { height: 42, confirmations: '3 confirmations', hash: 'abc…xyz' }),
    ).toBe('Block 42 · 3 confirmations · abc…xyz');
  });

  it('inserts replacement values literally (no RegExp special-char pitfalls)', () => {
    // A `$&`/`$1` in the substituted value must not be re-interpreted.
    expect(t('claim.hint', 'en', { amount: '$1 & $&' })).toContain('$1 & $&');
  });

  it('falls back to en for an unsupported locale or a missing key', () => {
    // `narrowLocale` is the runtime gate, but `t` is defensive too: an
    // unsupported locale slipping through resolves against the en table.
    const unsupported = 'fr' as unknown as Parameters<typeof t>[1];
    expect(t('receive.heading', unsupported)).toBe('Receive BTH');
  });

  it('narrows an arbitrary MetaMask locale to a supported Snap locale', () => {
    expect(narrowLocale('en')).toBe('en');
    expect(narrowLocale('es')).toBe('es');
    expect(narrowLocale('zh')).toBe('zh');
    // Region subtags and case are normalized to the base language.
    expect(narrowLocale('zh-CN')).toBe('zh');
    expect(narrowLocale('ES')).toBe('es');
    // Anything the Snap does not ship, or a non-string, falls back.
    expect(narrowLocale('fr')).toBe(DEFAULT_LOCALE);
    expect(narrowLocale('de-DE')).toBe(DEFAULT_LOCALE);
    expect(narrowLocale(undefined)).toBe(DEFAULT_LOCALE);
    expect(narrowLocale(42)).toBe(DEFAULT_LOCALE);
  });

  it('pluralizes the confirmation phrase Intl-free per locale', () => {
    // English: singular vs plural.
    expect(confirmationsPhrase(1, 'en')).toBe('1 confirmation');
    expect(confirmationsPhrase(3, 'en')).toBe('3 confirmations');
    // Spanish: singular vs plural.
    expect(confirmationsPhrase(1, 'es')).toBe('1 confirmación');
    expect(confirmationsPhrase(2, 'es')).toBe('2 confirmaciones');
    // Chinese has no grammatical plural — one form for any count.
    expect(confirmationsPhrase(1, 'zh')).toBe('1 个确认');
    expect(confirmationsPhrase(5, 'zh')).toBe('5 个确认');
  });

  it('has full key parity across every shipped locale (missing key fails CI)', () => {
    const enKeys = Object.keys(en).sort();
    for (const [name, table] of [
      ['es', es],
      ['zh', zh],
    ] as const) {
      const keys = Object.keys(table).sort();
      expect({ locale: name, keys }).toStrictEqual({ locale: name, keys: enKeys });
      // No empty translations slipped in.
      for (const key of enKeys) {
        expect((table as Record<string, string>)[key].length).toBeGreaterThan(0);
      }
    }
  });

  it('ships the same supported locales as the web wallet (en + es + zh)', () => {
    expect([...SUPPORTED_LOCALES]).toStrictEqual(['en', 'es', 'zh']);
    // Every supported locale has a message table.
    for (const locale of SUPPORTED_LOCALES) {
      expect(t('balance.heading', locale).length).toBeGreaterThan(0);
    }
  });

  it('keeps every en key non-empty (source-of-truth sanity)', () => {
    for (const key of Object.keys(en) as MessageKey[]) {
      expect(t(key, 'en').length).toBeGreaterThan(0);
    }
  });
});

describe('i18n: localized dialog rendering (SES harness via snap_getPreferences)', () => {
  /** Render the receive dialog under a given MetaMask locale preference. */
  async function receiveHeadingUnderLocale(locale?: string): Promise<string> {
    // `installSnap({ options: { locale } })` sets the simulation's MetaMask user
    // preference, which the harness surfaces through `snap_getPreferences` — the
    // runtime signal the Snap reads. (The Snap's manifest declares the
    // `snap_getPreferences` restricted permission, so the read is granted.)
    const { request } =
      locale === undefined
        ? await installSnap()
        : await installSnap({ options: { locale } });
    const response = request({ method: 'botho_showReceive' });
    const ui = await response.getInterface();
    expect(ui.type).toBe('alert');
    const rendered = JSON.stringify(ui.content);
    await (ui as { ok(): Promise<void> }).ok();
    await response;
    return rendered;
  }

  it('renders the Spanish heading when the user preference is es', async () => {
    const rendered = await receiveHeadingUnderLocale('es');
    expect(rendered).toContain('Recibir BTH');
    expect(rendered).not.toContain('Receive BTH');
  });

  it('renders the Chinese heading when the user preference is zh', async () => {
    const rendered = await receiveHeadingUnderLocale('zh');
    expect(rendered).toContain('接收 BTH');
    expect(rendered).not.toContain('Receive BTH');
  });

  it('falls back to the English heading for an unsupported MetaMask locale', async () => {
    const rendered = await receiveHeadingUnderLocale('fr');
    expect(rendered).toContain('Receive BTH');
  });

  it('renders the English heading by default (no regression for en users)', async () => {
    const rendered = await receiveHeadingUnderLocale();
    expect(rendered).toContain('Receive BTH');
  });
});
