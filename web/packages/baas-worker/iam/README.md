# Provisioner IAM policy (SEC / #508, #458 §5)

`provisioner-policy.json` is the **dedicated, least-privilege IAM policy** for the
Botho-as-a-Service control-plane provisioner. It is deliberately **tighter than
`botho-deploy`** and is the committed source of truth for what the worker's AWS
credential may do.

## How to apply

Create a **dedicated IAM user** (not `botho-deploy`), attach this policy, mint an
access key, and store it as Worker secrets:

```bash
# Replace the placeholders in provisioner-policy.json first (see below).
aws iam create-user --user-name botho-baas-provisioner
aws iam put-user-policy \
  --user-name botho-baas-provisioner \
  --policy-name BothoBaasProvisionerLeastPrivilege \
  --policy-document file://provisioner-policy.json
aws iam create-access-key --user-name botho-baas-provisioner

# then, in web/packages/baas-worker:
wrangler secret put AWS_ACCESS_KEY_ID
wrangler secret put AWS_SECRET_ACCESS_KEY
```

### Placeholders to replace

- `ACCOUNT_ID` — your 12-digit AWS account id.
- `SUBNET_ID` — the subnet the seed/faucet rigs run in (e.g. `subnet-0abc…`).

The `us-west-2` region, the `t4g.medium` instance type, the AMI
(`ami-012798e88aebdba5c`), the security group (`sg-0dd3fc95ec3916a4a`), and the
key-pair (`botho-nodes`) match the proven recipe and `rig-config.ts`. Expand the
region/AMI deliberately if the allowlist grows.

## What it allows — and what it forbids

Only **four EC2 verbs** plus tag-on-create. **No IAM, no S3, no broad EC2.** The
Cloudflare DNS write is a separate Cloudflare API token (`CF_DNS_API_TOKEN`), not
an AWS permission, so it is out of scope for this IAM policy.

| Sid | Action | Constraint |
|-----|--------|------------|
| `DescribeManagedRigs` | `ec2:DescribeInstances` | `Resource: *` (AWS cannot resource-scope Describe). Read-only; the worker further filters on the `botho:managed-rig` tag. |
| `RunManagedRigConstrained` | `ec2:RunInstances` | `ec2:InstanceType == t4g.medium`, `ec2:Region == us-west-2`, **required tag** `aws:RequestTag/botho:managed-rig == true`, and `aws:TagKeys` restricted to the four `botho:*` tags. |
| `RunManagedRigSupportingResources` | `ec2:RunInstances` | Pinned to the specific **AMI / security-group / subnet / key-pair** ARNs (plus the ENIs/volumes a launch creates). Off-recipe launches are denied. |
| `TagOnlyAtLaunch` | `ec2:CreateTags` | Allowed **only** when `ec2:CreateAction == RunInstances` (tag-on-create). The worker cannot re-tag arbitrary existing resources. |
| `TerminateOnlyManagedRigs` | `ec2:TerminateInstances` | **`ec2:ResourceTag/botho:managed-rig == true`** — terminate is restricted to instances already carrying the managed-rig tag. |

### Why the seed / seed2 / faucet nodes are safe

This is the single most important property (#458 §5):

1. **Terminate is tag-conditioned.** `TerminateOnlyManagedRigs` only permits
   `ec2:TerminateInstances` on resources where `ec2:ResourceTag/botho:managed-rig`
   is `true`. The seed, seed2, and faucet nodes do **not** carry that tag, so this
   credential can **never** terminate them — even if the worker is fully
   compromised.
2. **The tag cannot be forged onto them.** `CreateTags` is allowed **only** as
   part of a `RunInstances` call (`ec2:CreateAction == RunInstances`). The
   provisioner therefore cannot add `botho:managed-rig=true` to an existing
   seed/faucet node to make it terminable.
3. **The reconciliation sweep only sees managed rigs.** `describeManagedRigs`
   (the cron's only listing) filters on `tag:botho:managed-rig=true`, so the
   sweep cannot even enumerate the seed/faucet nodes, let alone reap one.

Together these are belt-and-suspenders: IAM forbids it, the tag can't be forged,
and the application code never targets a non-managed instance.

## Tests

`src/iam-policy.test.ts` loads this file and asserts:

- it is valid JSON with the IAM policy shape (`Version`, `Statement[]`),
- the action set is exactly the four allowed EC2 verbs (no IAM/S3/broad EC2),
- the `TerminateInstances` statement carries the
  `ec2:ResourceTag/botho:managed-rig == true` condition (the load-bearing
  guarantee), and
- `RunInstances` is constrained to `t4g.medium` + `us-west-2` + the required tag.
