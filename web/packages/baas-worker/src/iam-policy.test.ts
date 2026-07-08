import { describe, it, expect } from 'vitest'
import policy from '../iam/provisioner-policy.json'

/**
 * Validates the committed provisioner IAM policy (#508, #458 §5). The policy is a
 * deliverable artifact; these checks assert it stays valid JSON AND keeps the
 * load-bearing security properties — most importantly that TerminateInstances is
 * tag-conditioned to `botho:managed-node=true` so the credential can NEVER
 * terminate the seed/seed2/faucet nodes.
 *
 * The JSON is imported as a module (resolveJsonModule), so a parse failure or a
 * shape regression is caught at type-check time too — no fs/node dependency.
 */

interface Statement {
  Sid?: string
  Effect: string
  Action: string | string[]
  Resource: string | string[]
  Condition?: Record<string, Record<string, unknown>>
}

const statements = policy.Statement as unknown as Statement[]

function actionsOf(s: Statement): string[] {
  return Array.isArray(s.Action) ? s.Action : [s.Action]
}

describe('provisioner IAM policy', () => {
  it('has the IAM policy shape', () => {
    expect(policy.Version).toBe('2012-10-17')
    expect(Array.isArray(statements)).toBe(true)
    expect(statements.length).toBeGreaterThan(0)
    for (const s of statements) {
      expect(s.Effect).toBe('Allow')
      expect(s.Action).toBeDefined()
      expect(s.Resource).toBeDefined()
    }
  })

  it('does NOT contain IAM "Comment" keys (IAM would reject them)', () => {
    const raw = JSON.stringify(policy)
    expect(raw).not.toContain('"Comment"')
  })

  it('grants ONLY the four allowed EC2 verbs (no IAM, no S3, no broad EC2)', () => {
    const allActions = new Set(statements.flatMap(actionsOf))
    expect(allActions).toEqual(
      new Set([
        'ec2:DescribeInstances',
        'ec2:RunInstances',
        'ec2:CreateTags',
        'ec2:TerminateInstances',
      ]),
    )
    for (const a of allActions) {
      expect(a.startsWith('iam:')).toBe(false)
      expect(a.startsWith('s3:')).toBe(false)
      expect(a).not.toBe('ec2:*')
      expect(a).not.toBe('*')
    }
  })

  it('restricts TerminateInstances to botho:managed-node=true resources (CRITICAL)', () => {
    const term = statements.find((s) =>
      actionsOf(s).includes('ec2:TerminateInstances'),
    )
    expect(term).toBeDefined()
    const cond = term?.Condition?.StringEquals as Record<string, unknown> | undefined
    expect(cond).toBeDefined()
    // The load-bearing guarantee: terminate is gated on the managed-node tag, so
    // the seed/seed2/faucet nodes (which lack it) can never be terminated.
    expect(cond?.['ec2:ResourceTag/botho:managed-node']).toBe('true')
  })

  it('constrains RunInstances to t4g.medium + us-west-2 + the required tag', () => {
    const runStmts = statements.filter((s) =>
      actionsOf(s).includes('ec2:RunInstances'),
    )
    expect(runStmts.length).toBeGreaterThan(0)
    const constrained = runStmts.find((s) => s.Condition?.StringEquals !== undefined)
    expect(constrained).toBeDefined()
    const eq = constrained?.Condition?.StringEquals as Record<string, unknown>
    expect(eq['ec2:InstanceType']).toBe('t4g.medium')
    expect(eq['ec2:Region']).toBe('us-west-2')
    expect(eq['aws:RequestTag/botho:managed-node']).toBe('true')
  })

  it('allows CreateTags ONLY as part of a RunInstances launch (tag-on-create)', () => {
    const tag = statements.find((s) => actionsOf(s).includes('ec2:CreateTags'))
    expect(tag).toBeDefined()
    const eq = tag?.Condition?.StringEquals as Record<string, unknown> | undefined
    expect(eq?.['ec2:CreateAction']).toBe('RunInstances')
  })
})
