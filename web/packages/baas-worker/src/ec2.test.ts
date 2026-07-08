import { describe, it, expect, vi } from 'vitest'
import {
  buildRunInstancesBody,
  HttpEc2Client,
  isLiveInstanceState,
  parseDescribeInstancesResponse,
  parseRunInstancesResponse,
  type RunInstanceParams,
} from './ec2'

const PARAMS: RunInstanceParams = {
  region: 'us-west-2',
  amiId: 'ami-012798e88aebdba5c',
  instanceType: 't4g.medium',
  securityGroupId: 'sg-0dd3fc95ec3916a4a',
  keyName: 'botho-nodes',
  userDataBase64: 'BASE64DATA',
  tags: {
    'botho:managed-node': 'true',
    'botho:subscription': 'sub_ABC',
    'botho:user': 'cus_XYZ',
  },
}

describe('buildRunInstancesBody', () => {
  const body = buildRunInstancesBody(PARAMS)

  it('sets the RunInstances action + version + compute shape', () => {
    expect(body.get('Action')).toBe('RunInstances')
    expect(body.get('Version')).toBe('2016-11-15')
    expect(body.get('ImageId')).toBe('ami-012798e88aebdba5c')
    expect(body.get('InstanceType')).toBe('t4g.medium')
    expect(body.get('SecurityGroupId.1')).toBe('sg-0dd3fc95ec3916a4a')
    expect(body.get('KeyName')).toBe('botho-nodes')
    expect(body.get('MinCount')).toBe('1')
    expect(body.get('MaxCount')).toBe('1')
    expect(body.get('UserData')).toBe('BASE64DATA')
  })

  it('encodes the instance tags as TagSpecification entries', () => {
    expect(body.get('TagSpecification.1.ResourceType')).toBe('instance')
    expect(body.get('TagSpecification.1.Tag.1.Key')).toBe('botho:managed-node')
    expect(body.get('TagSpecification.1.Tag.1.Value')).toBe('true')
    expect(body.get('TagSpecification.1.Tag.2.Key')).toBe('botho:subscription')
    expect(body.get('TagSpecification.1.Tag.2.Value')).toBe('sub_ABC')
  })
})

describe('parseRunInstancesResponse', () => {
  it('extracts the instance id, state, and public ip', () => {
    const xml = `<?xml version="1.0"?>
      <RunInstancesResponse>
        <instancesSet><item>
          <instanceId>i-0abc123</instanceId>
          <instanceState><code>0</code><name>pending</name></instanceState>
          <ipAddress>203.0.113.7</ipAddress>
        </item></instancesSet>
      </RunInstancesResponse>`
    const inst = parseRunInstancesResponse(xml)
    expect(inst.instanceId).toBe('i-0abc123')
    expect(inst.state).toBe('pending')
    expect(inst.publicIp).toBe('203.0.113.7')
  })

  it('throws when there is no instance id', () => {
    expect(() => parseRunInstancesResponse('<Response/>')).toThrow()
  })
})

describe('parseDescribeInstancesResponse', () => {
  it('extracts instances + the botho:subscription tag', () => {
    const xml = `<DescribeInstancesResponse><reservationSet><item>
      <instancesSet>
        <item>
          <instanceId>i-111</instanceId>
          <instanceState><name>running</name></instanceState>
          <ipAddress>198.51.100.1</ipAddress>
          <tagSet>
            <item><key>botho:subscription</key><value>sub_ABC</value></item>
          </tagSet>
        </item>
      </instancesSet>
    </item></reservationSet></DescribeInstancesResponse>`
    const list = parseDescribeInstancesResponse(xml)
    expect(list).toHaveLength(1)
    expect(list[0].instanceId).toBe('i-111')
    expect(list[0].state).toBe('running')
    expect(list[0].subscriptionTag).toBe('sub_ABC')
  })

  it('extracts the botho:node-id tag and parses launchTime to epoch ms', () => {
    const xml = `<DescribeInstancesResponse><reservationSet><item>
      <instancesSet>
        <item>
          <instanceId>i-222</instanceId>
          <instanceState><name>pending</name></instanceState>
          <launchTime>2026-06-21T12:00:00.000Z</launchTime>
          <tagSet>
            <item><key>botho:managed-node</key><value>true</value></item>
            <item><key>botho:subscription</key><value>sub_DEF</value></item>
            <item><key>botho:node-id</key><value>def456</value></item>
          </tagSet>
        </item>
      </instancesSet>
    </item></reservationSet></DescribeInstancesResponse>`
    const list = parseDescribeInstancesResponse(xml)
    expect(list).toHaveLength(1)
    expect(list[0].subscriptionTag).toBe('sub_DEF')
    expect(list[0].nodeIdTag).toBe('def456')
    expect(list[0].launchTimeMs).toBe(Date.parse('2026-06-21T12:00:00.000Z'))
  })

  it('leaves launchTimeMs undefined when the field is absent', () => {
    const xml = `<instancesSet><item><instanceId>i-333</instanceId>
      <instanceState><name>running</name></instanceState></item></instancesSet>`
    const list = parseDescribeInstancesResponse(xml)
    expect(list[0].launchTimeMs).toBeUndefined()
  })

  it('returns an empty array when nothing matches', () => {
    expect(parseDescribeInstancesResponse('<Response/>')).toEqual([])
  })
})

describe('isLiveInstanceState', () => {
  it('treats running/pending/stopped as live', () => {
    expect(isLiveInstanceState('running')).toBe(true)
    expect(isLiveInstanceState('pending')).toBe(true)
    expect(isLiveInstanceState('stopped')).toBe(true)
  })
  it('treats terminated/shutting-down as not live', () => {
    expect(isLiveInstanceState('terminated')).toBe(false)
    expect(isLiveInstanceState('shutting-down')).toBe(false)
  })
})

describe('HttpEc2Client (mocked fetch — no real AWS)', () => {
  const creds = { accessKeyId: 'AKID', secretAccessKey: 'SECRET' }

  it('signs + POSTs RunInstances and parses the response', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(
        `<RunInstancesResponse><instancesSet><item><instanceId>i-xyz</instanceId>` +
          `<instanceState><name>pending</name></instanceState></item></instancesSet></RunInstancesResponse>`,
        { status: 200 },
      ),
    )
    const client = new HttpEc2Client(creds, fetchMock as unknown as typeof fetch)
    const inst = await client.runInstance(PARAMS)
    expect(inst.instanceId).toBe('i-xyz')

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://ec2.us-west-2.amazonaws.com/')
    const headers = init.headers as Record<string, string>
    expect(headers.Authorization).toContain('AWS4-HMAC-SHA256')
  })

  it('describeManagedNodes filters on the botho:managed-node=true tag', async () => {
    let sentBody = ''
    const fetchMock = vi.fn(async (_url: string, init: RequestInit) => {
      sentBody = String(init.body)
      return new Response('<DescribeInstancesResponse/>', { status: 200 })
    })
    const client = new HttpEc2Client(creds, fetchMock as unknown as typeof fetch)
    await client.describeManagedNodes('us-west-2')
    const params = new URLSearchParams(sentBody)
    expect(params.get('Action')).toBe('DescribeInstances')
    expect(params.get('Filter.1.Name')).toBe('tag:botho:managed-node')
    expect(params.get('Filter.1.Value.1')).toBe('true')
  })

  it('throws Ec2Error on an AWS error response', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(
        `<Response><Errors><Error><Code>UnauthorizedOperation</Code><Message>nope</Message></Error></Errors></Response>`,
        { status: 403 },
      ),
    )
    const client = new HttpEc2Client(creds, fetchMock as unknown as typeof fetch)
    await expect(client.runInstance(PARAMS)).rejects.toThrow(/UnauthorizedOperation/)
  })
})
