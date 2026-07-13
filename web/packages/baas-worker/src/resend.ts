/**
 * Resend transactional-email client for the status-link delivery (#805 part 2,
 * #458 §4).
 *
 * On the FIRST successful provision (a fresh D1 insert, not a webhook replay) the
 * webhook path mails the customer their magic-link status URL, so they don't have
 * to keep the success-page tab open while the node comes up. This module is the
 * pure, injectable core of that send:
 *
 *   - `fetchImpl` defaults to `boundFetch` (workerd requires the bound global — a
 *     bare `fetch` throws `Illegal invocation` in production; see `bound-fetch.ts`),
 *   - it is ENV-GATED at the call site: with `RESEND_API_KEY` unset the webhook
 *     skips the send entirely (log-and-skip, never a 500, never a blocked ACK to
 *     Stripe), so the Worker functions fully without Resend configured,
 *   - it never throws to the webhook: a send failure is surfaced as a returned
 *     error so the caller logs it and still ACKs Stripe (provisioning already
 *     succeeded; the reconciliation cron + the on-page link are the safety nets).
 *
 * No secret lives in the repo — `RESEND_API_KEY` and the verified `botho.io`
 * sender address (`RESEND_FROM_ADDRESS`) are Worker secrets / vars, supplied by
 * the operator once the domain is verified.
 */

import { boundFetch } from './bound-fetch'

/** Default sender if `RESEND_FROM_ADDRESS` is unset (must be a verified domain). */
export const DEFAULT_STATUS_FROM_ADDRESS = 'Botho <nodes@botho.io>'

/** Inputs for the status-link email. */
export interface StatusLinkEmail {
  /** Recipient (the Stripe customer's email). */
  to: string
  /** The `/node/status?token=…` URL the customer opens. */
  statusUrl: string
  /** Optional Stripe Customer Portal URL for the cancel-anytime note. */
  manageUrl?: string
}

/** Outcome of a send attempt. Never throws — the caller logs and ACKs Stripe. */
export type SendResult =
  | { ok: true; id?: string }
  | { ok: false; error: string; status?: number }

/**
 * Build the plain-text + minimal-HTML body for the status-link email. Kept pure
 * so the copy can be asserted in tests without any network I/O.
 */
export function buildStatusLinkEmailBody(email: StatusLinkEmail): {
  subject: string
  text: string
  html: string
} {
  const subject = 'Your Botho node is on its way'
  const manageLine = email.manageUrl
    ? `\n\nManage or cancel your subscription anytime: ${email.manageUrl}`
    : '\n\nYou can cancel anytime from your node status page.'

  const text =
    `Thanks for subscribing to a managed Botho node.\n\n` +
    `Your node is being provisioned now. Open your private status page to see ` +
    `its RPC endpoint, live health, and a one-click link to open it in the wallet:\n\n` +
    `${email.statusUrl}\n\n` +
    `This link is your access to the node — keep it private.` +
    manageLine

  const manageHtml = email.manageUrl
    ? `<p style="color:#6b7280;font-size:14px">Manage or cancel your subscription anytime: ` +
      `<a href="${email.manageUrl}">billing portal</a>.</p>`
    : `<p style="color:#6b7280;font-size:14px">You can cancel anytime from your node status page.</p>`

  const html =
    `<p>Thanks for subscribing to a managed Botho node.</p>` +
    `<p>Your node is being provisioned now. Open your private status page to see ` +
    `its RPC endpoint, live health, and a one-click link to open it in the wallet:</p>` +
    `<p><a href="${email.statusUrl}">View your node status</a></p>` +
    `<p style="color:#6b7280;font-size:14px">This link is your access to the node — keep it private.</p>` +
    manageHtml

  return { subject, text, html }
}

/**
 * Send the status-link email via the Resend REST API
 * (`POST https://api.resend.com/emails`).
 *
 * `fetchImpl` is injectable (defaults to `boundFetch`) so tests assert on the
 * exact request without network I/O. Returns a discriminated result instead of
 * throwing so the webhook can log a failure and still ACK Stripe.
 */
export async function sendStatusLinkEmail(
  email: StatusLinkEmail,
  opts: { apiKey: string; from?: string; fetchImpl?: typeof fetch },
): Promise<SendResult> {
  const fetchImpl = opts.fetchImpl ?? boundFetch
  const from = opts.from && opts.from.length > 0 ? opts.from : DEFAULT_STATUS_FROM_ADDRESS
  const { subject, text, html } = buildStatusLinkEmailBody(email)

  let resp: Response
  try {
    resp = await fetchImpl('https://api.resend.com/emails', {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${opts.apiKey}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ from, to: email.to, subject, text, html }),
    })
  } catch {
    return { ok: false, error: 'could not reach Resend' }
  }

  const json = (await resp.json().catch(() => ({}))) as {
    id?: string
    message?: string
    error?: { message?: string }
  }

  if (!resp.ok) {
    return {
      ok: false,
      error: json.error?.message ?? json.message ?? `Resend returned HTTP ${resp.status}`,
      status: resp.status,
    }
  }
  return { ok: true, id: json.id }
}

/**
 * Retrieve a Stripe customer's email address (needed as the email recipient —
 * the D1 node row stores the customer id, not the email). Returns `undefined`
 * on any error / a customer without an email so the caller simply skips the send
 * rather than failing the webhook. `fetchImpl` defaults to `boundFetch`.
 */
export async function retrieveCustomerEmail(
  customerId: string,
  stripeSecretKey: string,
  fetchImpl: typeof fetch = boundFetch,
): Promise<string | undefined> {
  try {
    const resp = await fetchImpl(
      `https://api.stripe.com/v1/customers/${encodeURIComponent(customerId)}`,
      {
        method: 'GET',
        headers: {
          Authorization: `Bearer ${stripeSecretKey}`,
          'Stripe-Version': '2024-06-20',
        },
      },
    )
    if (!resp.ok) return undefined
    const json = (await resp.json().catch(() => ({}))) as { email?: string }
    return json.email && json.email.length > 0 ? json.email : undefined
  } catch {
    return undefined
  }
}
