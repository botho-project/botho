// ============================================================================
// Amount Formatting Utilities
// ============================================================================

/**
 * Number of decimal places for BTH amounts.
 * 1 BTH = 1,000,000,000,000 picocredits (12 decimals, like Monero)
 */
export const BTH_DECIMALS = 12
export const BTH_MULTIPLIER = BigInt(10 ** BTH_DECIMALS)

/**
 * Format a BTH amount from picocredits to human-readable string.
 *
 * @param picocredits - Amount in smallest unit
 * @param options - Formatting options
 * @returns Formatted string (e.g., "1,234.567")
 */
export function formatBTH(
  picocredits: bigint,
  options: {
    /** Minimum fraction digits (default: 2) */
    minDecimals?: number
    /** Maximum fraction digits (default: 6) */
    maxDecimals?: number
    /** Include thousands separators (default: true) */
    separators?: boolean
  } = {}
): string {
  const { minDecimals = 2, maxDecimals = 6, separators = true } = options

  const credits = Number(picocredits) / Number(BTH_MULTIPLIER)

  if (separators) {
    return credits.toLocaleString(undefined, {
      minimumFractionDigits: minDecimals,
      maximumFractionDigits: maxDecimals,
    })
  }

  return credits.toFixed(maxDecimals).replace(/\.?0+$/, '')
}

/**
 * Parse a BTH string to picocredits.
 *
 * @param str - Amount string (e.g., "1,234.56" or "1234.56")
 * @returns Amount in picocredits
 * @throws Error if string is invalid
 */
export function parseBTH(str: string): bigint {
  // Remove commas and whitespace
  const cleaned = str.replace(/,/g, '').trim()

  if (!cleaned || cleaned === '.') {
    return BigInt(0)
  }

  // Validate format
  if (!/^-?\d*\.?\d*$/.test(cleaned)) {
    throw new Error(`Invalid BTH amount: ${str}`)
  }

  const [whole = '0', fraction = ''] = cleaned.split('.')

  // Parse whole part
  const wholeAmount = BigInt(whole || '0') * BTH_MULTIPLIER

  // Parse fraction part (pad or truncate to 12 digits)
  const fractionPadded = fraction.padEnd(BTH_DECIMALS, '0').slice(0, BTH_DECIMALS)
  const fractionAmount = BigInt(fractionPadded)

  return wholeAmount + fractionAmount
}

/**
 * Format a BTH amount with currency symbol.
 *
 * @param picocredits - Amount in smallest unit
 * @param options - Formatting options
 * @returns Formatted string with BTH suffix (e.g., "1,234.56 BTH")
 */
export function formatBTHWithSymbol(
  picocredits: bigint,
  options: Parameters<typeof formatBTH>[1] = {}
): string {
  return `${formatBTH(picocredits, options)} BTH`
}

/**
 * Format a signed BTH amount (for transactions).
 *
 * @param picocredits - Amount in smallest unit
 * @param isPositive - Whether to show + prefix
 * @returns Formatted string with sign (e.g., "+1,234.56" or "-1,234.56")
 */
export function formatSignedBTH(picocredits: bigint, isPositive: boolean): string {
  const prefix = isPositive ? '+' : '-'
  return `${prefix}${formatBTH(picocredits)}`
}
