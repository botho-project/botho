/**
 * Cloudflare DNS client for the Botho-as-a-Service provisioner (#458 §3 step 3).
 *
 * Creates / deletes the `A node-<id>.testnet.botho.io -> <instance public IP>`
 * record via the Cloudflare API using a Zone:DNS:Edit token. The provisioner
 * depends on the `DnsClient` *interface* so tests use an in-memory fake and no
 * real Cloudflare call happens under test (#502 requirement).
 *
 * Secrets: the API token + zone id come from Worker secrets / vars — never the
 * repo (#458 §2, §5).
 */

/** A managed DNS A record. */
export interface DnsRecord {
  id: string
  name: string
  content: string
}

/** Injectable Cloudflare DNS surface. */
export interface DnsClient {
  /**
   * Idempotently ensure `A <name> -> <ip>` exists. If a record with `name`
   * already exists it is updated to `ip`; otherwise it is created. Returns the
   * record. Idempotency matters because the provisioner may retry (#458 §3).
   */
  upsertARecord(name: string, ip: string): Promise<DnsRecord>
  /** Delete the A record for `name` if present (teardown). No-op if absent. */
  deleteARecord(name: string): Promise<void>
}

const CF_API = 'https://api.cloudflare.com/client/v4'

interface CfListResponse {
  success: boolean
  errors?: { message?: string }[]
  result?: { id: string; name: string; content: string }[]
}

interface CfRecordResponse {
  success: boolean
  errors?: { message?: string }[]
  result?: { id: string; name: string; content: string }
}

/** Error from the Cloudflare DNS API. */
export class DnsError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message)
    this.name = 'DnsError'
  }
}

/**
 * Real Cloudflare DNS client. `fetchImpl` is injectable (defaults to global
 * fetch). The provisioner's tests use the fake instead, so this never runs under
 * test.
 */
export class HttpDnsClient implements DnsClient {
  constructor(
    private readonly apiToken: string,
    private readonly zoneId: string,
    private readonly fetchImpl: typeof fetch = fetch,
  ) {}

  private headers(): Record<string, string> {
    return {
      Authorization: `Bearer ${this.apiToken}`,
      'Content-Type': 'application/json',
    }
  }

  private async findByName(name: string): Promise<DnsRecord | undefined> {
    const url = `${CF_API}/zones/${this.zoneId}/dns_records?type=A&name=${encodeURIComponent(name)}`
    const resp = await this.fetchImpl(url, { method: 'GET', headers: this.headers() })
    const json = (await resp.json()) as CfListResponse
    if (!resp.ok || !json.success) {
      throw new DnsError(
        json.errors?.[0]?.message ?? `Cloudflare list failed (HTTP ${resp.status})`,
        resp.status,
      )
    }
    const rec = json.result?.[0]
    return rec ? { id: rec.id, name: rec.name, content: rec.content } : undefined
  }

  async upsertARecord(name: string, ip: string): Promise<DnsRecord> {
    const existing = await this.findByName(name)
    const payload = JSON.stringify({ type: 'A', name, content: ip, ttl: 120, proxied: false })

    const url = existing
      ? `${CF_API}/zones/${this.zoneId}/dns_records/${existing.id}`
      : `${CF_API}/zones/${this.zoneId}/dns_records`
    const resp = await this.fetchImpl(url, {
      method: existing ? 'PUT' : 'POST',
      headers: this.headers(),
      body: payload,
    })
    const json = (await resp.json()) as CfRecordResponse
    if (!resp.ok || !json.success || !json.result) {
      throw new DnsError(
        json.errors?.[0]?.message ?? `Cloudflare upsert failed (HTTP ${resp.status})`,
        resp.status,
      )
    }
    return { id: json.result.id, name: json.result.name, content: json.result.content }
  }

  async deleteARecord(name: string): Promise<void> {
    const existing = await this.findByName(name)
    if (!existing) return
    const resp = await this.fetchImpl(
      `${CF_API}/zones/${this.zoneId}/dns_records/${existing.id}`,
      { method: 'DELETE', headers: this.headers() },
    )
    if (!resp.ok) {
      const json = (await resp.json().catch(() => ({}))) as CfRecordResponse
      throw new DnsError(
        json.errors?.[0]?.message ?? `Cloudflare delete failed (HTTP ${resp.status})`,
        resp.status,
      )
    }
  }
}
