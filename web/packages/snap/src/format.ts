/**
 * SES-safe amount formatting helpers for the Snap dialogs.
 *
 * `@botho/core`'s `formatBTH` uses `Number#toLocaleString` (Intl), which is not
 * reliably endowed in the Snaps SES executor. The Snap only needs a plain,
 * deterministic decimal string for its confirmation UI, so it formats
 * picocredits itself with integer/bigint math — no `Intl`, no floats.
 */

/** picocredits per 1 BTH (12 decimals — matches `@botho/core` `BTH_DECIMALS`). */
export const BTH_DECIMALS = 12n;
export const PICOCREDITS_PER_BTH = 10n ** BTH_DECIMALS;

/**
 * Format a picocredit amount as a fixed-point BTH string with a trailing-zero
 * trim (e.g. `1234500000000n` -> `"1.2345"`, `5000000000000n` -> `"5"`).
 * Deterministic and Intl-free so it is safe inside the SES sandbox.
 */
export function formatPicocreditsBTH(picocredits: bigint): string {
  const neg = picocredits < 0n;
  const abs = neg ? -picocredits : picocredits;
  const whole = abs / PICOCREDITS_PER_BTH;
  const frac = abs % PICOCREDITS_PER_BTH;

  let out = whole.toString();
  if (frac > 0n) {
    // Zero-pad the fractional part to the full decimal width, then trim
    // trailing zeros so "1.230000000000" renders as "1.23".
    const fracStr = frac.toString().padStart(Number(BTH_DECIMALS), '0').replace(/0+$/, '');
    out += `.${fracStr}`;
  }
  return neg ? `-${out}` : out;
}

/** As {@link formatPicocreditsBTH} but with the ` BTH` unit suffix. */
export function formatBTHWithUnit(picocredits: bigint): string {
  return `${formatPicocreditsBTH(picocredits)} BTH`;
}
