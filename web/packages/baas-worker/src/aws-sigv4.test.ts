import { describe, it, expect } from 'vitest'
import { amzDate, dateStamp, signAwsRequest } from './aws-sigv4'

const FIXED = new Date('2026-06-21T12:00:00.000Z')

describe('amzDate / dateStamp', () => {
  it('formats the SigV4 amz-date', () => {
    expect(amzDate(FIXED)).toBe('20260621T120000Z')
  })
  it('formats the credential-scope date stamp', () => {
    expect(dateStamp(FIXED)).toBe('20260621')
  })
})

describe('signAwsRequest', () => {
  const creds = { accessKeyId: 'AKIDEXAMPLE', secretAccessKey: 'wJalrSecret' }

  it('attaches Authorization + X-Amz-Date headers and the signed body', async () => {
    const signed = await signAwsRequest({
      endpoint: 'https://ec2.us-west-2.amazonaws.com/',
      region: 'us-west-2',
      service: 'ec2',
      body: 'Action=DescribeInstances&Version=2016-11-15',
      credentials: creds,
      date: FIXED,
    })

    expect(signed.method).toBe('POST')
    expect(signed.url).toBe('https://ec2.us-west-2.amazonaws.com/')
    expect(signed.body).toBe('Action=DescribeInstances&Version=2016-11-15')
    expect(signed.headers['X-Amz-Date']).toBe('20260621T120000Z')
    expect(signed.headers['Content-Type']).toBe('application/x-www-form-urlencoded')

    const auth = signed.headers.Authorization
    expect(auth).toContain('AWS4-HMAC-SHA256')
    expect(auth).toContain('Credential=AKIDEXAMPLE/20260621/us-west-2/ec2/aws4_request')
    expect(auth).toMatch(/SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date/)
    expect(auth).toMatch(/Signature=[0-9a-f]{64}/)
  })

  it('is deterministic for the same inputs', async () => {
    const opts = {
      endpoint: 'https://ec2.us-west-2.amazonaws.com/',
      region: 'us-west-2',
      service: 'ec2',
      body: 'Action=DescribeInstances',
      credentials: creds,
      date: FIXED,
    }
    const a = await signAwsRequest(opts)
    const b = await signAwsRequest(opts)
    expect(a.headers.Authorization).toBe(b.headers.Authorization)
  })

  it('includes the security token in headers + signed headers when present', async () => {
    const signed = await signAwsRequest({
      endpoint: 'https://ec2.us-west-2.amazonaws.com/',
      region: 'us-west-2',
      service: 'ec2',
      body: 'Action=DescribeInstances',
      credentials: { ...creds, sessionToken: 'TEMPTOKEN' },
      date: FIXED,
    })
    expect(signed.headers['X-Amz-Security-Token']).toBe('TEMPTOKEN')
    expect(signed.headers.Authorization).toContain('x-amz-security-token')
  })

  it('changes the signature when the body changes', async () => {
    const base = {
      endpoint: 'https://ec2.us-west-2.amazonaws.com/',
      region: 'us-west-2',
      service: 'ec2',
      credentials: creds,
      date: FIXED,
    }
    const a = await signAwsRequest({ ...base, body: 'Action=A' })
    const b = await signAwsRequest({ ...base, body: 'Action=B' })
    expect(a.headers.Authorization).not.toBe(b.headers.Authorization)
  })
})
