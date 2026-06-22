/**
 * Minimal AWS Signature Version 4 signer for the EC2 query API, using the Web
 * Crypto API available in Cloudflare Workers (no Node Buffer, no aws-sdk).
 *
 * We only need to sign `POST https://ec2.<region>.amazonaws.com/` requests whose
 * body is an `application/x-www-form-urlencoded` action query (e.g.
 * `Action=RunInstances&...`). This is the smallest dependency-free way to call
 * EC2 from a Worker.
 *
 * Reference: AWS SigV4 "Signing AWS API requests".
 *
 * Everything here is pure and deterministic given an explicit `date` + the
 * inputs, so the signing can be unit-tested without any network access.
 */

export interface AwsCredentials {
  accessKeyId: string
  secretAccessKey: string
  /** Optional STS session token (sets `X-Amz-Security-Token`). */
  sessionToken?: string
}

export interface SignedRequest {
  url: string
  method: 'POST'
  headers: Record<string, string>
  body: string
}

const encoder = new TextEncoder()

function toHex(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf)
  let out = ''
  for (let i = 0; i < bytes.length; i++) {
    out += bytes[i].toString(16).padStart(2, '0')
  }
  return out
}

async function sha256Hex(data: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', encoder.encode(data))
  return toHex(digest)
}

/** Copy any byte source into a fresh, plain ArrayBuffer (satisfies BufferSource). */
function toArrayBuffer(key: ArrayBuffer | Uint8Array): ArrayBuffer {
  const src = key instanceof Uint8Array ? key : new Uint8Array(key)
  const out = new ArrayBuffer(src.byteLength)
  new Uint8Array(out).set(src)
  return out
}

async function hmac(key: ArrayBuffer | Uint8Array, data: string): Promise<ArrayBuffer> {
  const cryptoKey = await crypto.subtle.importKey(
    'raw',
    toArrayBuffer(key),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  )
  return crypto.subtle.sign('HMAC', cryptoKey, encoder.encode(data))
}

/** Format a Date as the SigV4 `YYYYMMDDTHHMMSSZ` amz-date. */
export function amzDate(date: Date): string {
  return date.toISOString().replace(/[:-]|\.\d{3}/g, '')
}

/** The `YYYYMMDD` date stamp used in the credential scope. */
export function dateStamp(date: Date): string {
  return amzDate(date).slice(0, 8)
}

/**
 * Derive the SigV4 signing key for `date`/`region`/`service` from the secret
 * access key.
 */
async function signingKey(
  secretAccessKey: string,
  date: Date,
  region: string,
  service: string,
): Promise<ArrayBuffer> {
  const kDate = await hmac(encoder.encode(`AWS4${secretAccessKey}`), dateStamp(date))
  const kRegion = await hmac(kDate, region)
  const kService = await hmac(kRegion, service)
  return hmac(kService, 'aws4_request')
}

/**
 * Sign a `POST` form-body request to an AWS service endpoint with SigV4 and
 * return the request with the `Authorization` + `X-Amz-*` headers attached.
 *
 * `date` is injectable so the signature is deterministic in tests.
 */
export async function signAwsRequest(opts: {
  endpoint: string // e.g. https://ec2.us-west-2.amazonaws.com/
  region: string
  service: string // e.g. "ec2"
  body: string // application/x-www-form-urlencoded
  credentials: AwsCredentials
  date?: Date
}): Promise<SignedRequest> {
  const { endpoint, region, service, body, credentials } = opts
  const date = opts.date ?? new Date()
  const url = new URL(endpoint)
  const host = url.host
  const amz = amzDate(date)
  const stamp = dateStamp(date)

  const contentType = 'application/x-www-form-urlencoded'
  const payloadHash = await sha256Hex(body)

  // Canonical headers MUST be sorted lowercase by name. Include the security
  // token (if any) so it is part of the signature.
  const baseHeaders: Record<string, string> = {
    'content-type': contentType,
    host,
    'x-amz-content-sha256': payloadHash,
    'x-amz-date': amz,
  }
  if (credentials.sessionToken) {
    baseHeaders['x-amz-security-token'] = credentials.sessionToken
  }

  const sortedHeaderNames = Object.keys(baseHeaders).sort()
  const canonicalHeaders =
    sortedHeaderNames.map((n) => `${n}:${baseHeaders[n]}\n`).join('')
  const signedHeaders = sortedHeaderNames.join(';')

  const canonicalRequest = [
    'POST',
    url.pathname || '/',
    url.search.replace(/^\?/, ''), // canonical query string (empty for body POST)
    canonicalHeaders,
    signedHeaders,
    payloadHash,
  ].join('\n')

  const scope = `${stamp}/${region}/${service}/aws4_request`
  const stringToSign = [
    'AWS4-HMAC-SHA256',
    amz,
    scope,
    await sha256Hex(canonicalRequest),
  ].join('\n')

  const key = await signingKey(credentials.secretAccessKey, date, region, service)
  const signature = toHex(await hmac(key, stringToSign))

  const authorization =
    `AWS4-HMAC-SHA256 Credential=${credentials.accessKeyId}/${scope}, ` +
    `SignedHeaders=${signedHeaders}, Signature=${signature}`

  const headers: Record<string, string> = {
    'Content-Type': contentType,
    'X-Amz-Date': amz,
    'X-Amz-Content-Sha256': payloadHash,
    Authorization: authorization,
  }
  if (credentials.sessionToken) {
    headers['X-Amz-Security-Token'] = credentials.sessionToken
  }

  return { url: endpoint, method: 'POST', headers, body }
}
