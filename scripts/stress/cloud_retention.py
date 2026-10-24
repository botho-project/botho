#!/usr/bin/env python3
"""Retire archived or verified unused qualification fleets; age alone is insufficient.

Default: write a reviewable plan. --apply also executes it. The newest cloud
fleet is always retained, and automatic use should retain archives for 24h.
Only qualification fleets are eligible: the legacy Project=botho-qualification
tag, or project=botho with Purpose=qualification. No snapshots,
public nodes, build machines, or untagged resources are removed.
An undeployed fleet needs complete remote retirement proof instead of archives.
"""
import argparse
import collections
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

PROJECT = 'botho-qualification'
ROLES = {'controller', 'n1', 'n2', 'n3', 'n4', 'n5'}
# EC2 tag filters for each supported scheme; all filters in a row must match.
SCHEMES = (('Name=tag:Project,Values=' + PROJECT,),
           ('Name=tag:project,Values=botho', 'Name=tag:Purpose,Values=qualification'))


def need(value, reason):
    if not value:
        raise RuntimeError(reason)


def read(path):
    return json.loads(Path(path).read_text())


def save(path, value):
    path = Path(path)
    tmp = path.with_suffix(path.suffix + '.tmp')
    tmp.write_text(json.dumps(value, indent=2) + '\n')
    tmp.replace(path)


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def tags(value):
    return {t['Key']: t['Value'] for t in value.get('Tags', [])}


def qualification(t):
    return t.get('Project') == PROJECT or (t.get('project') == 'botho' and t.get('Purpose') == 'qualification')


def evidence(root):
    root = Path(root)
    need(any((root / name).is_file() for name in
             ('result.json', 'supervisor-held.json', 'retirement.json')), 'No terminal evidence')
    inventory = read(root / 'inventory.json')
    identities = {n['id']: n['role'] for n in inventory}
    need(len(inventory) == len(identities) == 6 and set(identities.values()) == ROLES,
         'Expected exactly six distinct roles and instances')
    final = read(root / 'fleet-final-state.json')
    need(set(final) == set(identities) and set(final.values()) <= {'stopped', 'terminated'},
         'Recorded fleet not retired')
    manifest_path = root / 'final-evidence-manifest.json'
    if not manifest_path.exists():
        retirement = read(root / 'retirement.json')
        need(retirement.get('state') == 'aborted_before_deployment', 'Missing workload archives')
        infra = read(root / 'infrastructure-plan.json')
        checks = retirement.get('remote_verification', {})
        need(set(checks) == ROLES, 'Incomplete unused-host verification')
        need(all(v.get('run') == infra['run_id'] and v.get('bootstrap') is True
                 and v.get('node_unit') is False and v.get('controller_data') is False
                 for v in checks.values()), 'Unused-host verification failed')
        need(not any((root / name).exists() for name in
                     ('preparation.intent', 'node-start.intent', 'warmup-start.intent',
                      'wallet-backup', 'common', 'result.json')), 'Deployment evidence exists')
        need(type(retirement.get('at')) in (int, float) and retirement['at'] > infra['created_at'],
             'Invalid unused-host verification time')
        return {'root': str(root), 'identities': identities, 'sha256': {},
                'unused_verified_at': retirement['at'], 'run': infra['run_id'],
                'proof_sha256': {name: digest(root / name) for name in
                                 ('retirement.json', 'infrastructure-plan.json', 'inventory.json', 'fleet-final-state.json')},
                'archived_at': (root / 'retirement.json').stat().st_mtime}
    manifest = read(manifest_path)
    need(set(manifest) == ROLES, 'Incomplete evidence manifest')
    for role, expected in manifest.items():
        need(digest(root / 'final-evidence' / (role + '.tar.gz')) == expected,
             'Archive checksum mismatch: ' + role)
    return {'root': str(root), 'identities': identities, 'sha256': manifest,
            'archived_at': (root / 'final-evidence-manifest.json').stat().st_mtime}


def make_plan(instances, volumes, candidates, now, min_age_hours):
    need(min_age_hours >= 0, 'Negative retention age')
    grouped = collections.defaultdict(list)
    for node in instances:
        t = tags(node)
        if qualification(t) and node['State']['Name'] != 'terminated':
            need(re.fullmatch(r'q[0-9]{8}-[a-z0-9-]+', t.get('Run', '')), 'Invalid run tag')
            grouped[t['Run']].append(node)
    # LaunchTime is refreshed by an EC2 restart: a restarted fleet is also protected.
    latest = max(grouped, key=lambda run: max(n['LaunchTime'] for n in grouped[run]), default=None)
    plan = {'at': now, 'protected_latest_run': latest, 'min_age_hours': min_age_hours,
            'fleets': [], 'retained': []}
    for run, nodes in sorted(grouped.items()):
        ids = {n['InstanceId'] for n in nodes}
        matches = [e for e in candidates if set(e['identities']) == ids]
        reason = None
        if run == latest:
            reason = 'Newest fleet retained for diagnosis'
        elif len(nodes) != 6 or {tags(n).get('Role') for n in nodes} != ROLES:
            reason = 'Incomplete or unexpected fleet'
        elif any(n['State']['Name'] != 'stopped' for n in nodes):
            reason = 'Fleet is not stopped'
        elif not matches:
            reason = 'No complete verified local archive'
        else:
            e = max(matches, key=lambda e: e['archived_at'])
            if now - e['archived_at'] < min_age_hours * 3600:
                reason = 'Archive retention window'
            elif any(e['identities'][n['InstanceId']] != tags(n)['Role'] for n in nodes):
                reason = 'Archive role mismatch'
            elif 'unused_verified_at' in e and (e['run'] != run or any(
                    datetime.datetime.fromisoformat(n['LaunchTime'].replace('Z', '+00:00')).timestamp()
                    > e['unused_verified_at'] for n in nodes)):
                reason = 'Unused-host proof predates last instance launch or has wrong run'
            else:
                vs = [v for v in volumes if any(a['InstanceId'] in ids for a in v['Attachments'])]
                expected_volumes = {b['Ebs']['VolumeId'] for n in nodes for b in n.get('BlockDeviceMappings', []) if 'Ebs' in b}
                if (len(vs) != 6 or {v['VolumeId'] for v in vs} != expected_volumes
                        or any(not qualification(tags(v)) or tags(v).get('Run') != run
                               or len(v['Attachments']) != 1 or v['State'] != 'in-use'
                               or tags(v).get('Role') != e['identities'].get(v['Attachments'][0]['InstanceId']) for v in vs)):
                    reason = 'Disk identity, attachment, or tag mismatch'
                elif any(len(n.get('BlockDeviceMappings', [])) != 1 for n in nodes):
                    reason = 'Unexpected extra disks'
                else:
                    plan['fleets'].append({'run': run, 'instance_ids': sorted(ids), 'evidence': e,
                                           'volumes': [{'id': v['VolumeId'], 'gib': v['Size'],
                                                        'instance_id': v['Attachments'][0]['InstanceId'],
                                                        'role': tags(v)['Role']} for v in sorted(vs, key=lambda v: v['VolumeId'])]})
        if reason:
            plan['retained'].append({'run': run, 'reason': reason})
    plan['volume_count'] = sum(len(f['volumes']) for f in plan['fleets'])
    plan['gib'] = sum(v['gib'] for f in plan['fleets'] for v in f['volumes'])
    return plan


class Aws:
    def __init__(self, profile, region):
        self.prefix = ['aws', '--profile', profile, '--region', region]

    def __call__(self, *args):
        raw = subprocess.check_output(self.prefix + list(args) + ['--output', 'json',
                                      '--cli-connect-timeout', '10', '--cli-read-timeout', '30'], timeout=60)
        return json.loads(raw) if raw.strip() else {}


def inventory(aws):
    nodes = {}
    for filters in SCHEMES:
        raw = aws('ec2', 'describe-instances', '--filters', *filters)
        nodes.update((n['InstanceId'], n) for r in raw['Reservations'] for n in r['Instances'])
    return list(nodes.values()), aws('ec2', 'describe-volumes')['Volumes']


def evidence_roots(state_roots):
    # Runs live in several operator directories; contents, not names, decide eligibility.
    seen = set()
    for base in state_roots:
        for root in [base, *sorted(base.iterdir())]:
            if (root.is_dir() and root.resolve() not in seen and any(
                    (root / name).is_file() for name in ('final-evidence-manifest.json', 'retirement.json'))):
                seen.add(root.resolve())
                yield root


def backup(fleet, destination):
    original = evidence(fleet['evidence']['root'])
    need(original['sha256'] == fleet['evidence']['sha256'], 'Evidence changed since planning')
    need(original['identities'] == fleet['evidence']['identities'], 'Archive identity changed')
    need(original.get('proof_sha256') == fleet['evidence'].get('proof_sha256'), 'Retirement proof changed')
    source = Path(original['root'])
    dest = Path(destination) / source.name
    need(dest.resolve() != source.resolve(), 'Backup must be independent of original directory')
    dest.mkdir(parents=True, exist_ok=True, mode=0o700)
    (dest / 'final-evidence').mkdir(exist_ok=True, mode=0o700)
    for role, expected in original['sha256'].items():
        target = dest / 'final-evidence' / (role + '.tar.gz')
        if not target.exists():
            tmp = target.with_suffix('.copying')
            shutil.copyfile(source / 'final-evidence' / target.name, tmp)
            need(digest(tmp) == expected, 'Backup copy failed checksum')
            tmp.replace(target)
        need(digest(target) == expected, 'Existing backup checksum mismatch')
    for name in ('inventory.json', 'fleet-final-state.json', 'final-evidence-manifest.json',
                 'result.json', 'supervisor-held.json', 'retirement.json', 'infrastructure-plan.json',
                 'prefund-manifest.json', 'rehearsal-report.json', 'wallet-backup-receipt.json'):
        if (source / name).is_file():
            shutil.copyfile(source / name, dest / name)
    need(evidence(dest)['sha256'] == original['sha256'], 'Independent archive verification failed')
    need(evidence(dest).get('proof_sha256') == original.get('proof_sha256'), 'Independent retirement proof mismatch')
    return str(dest)


def apply_plan(plan, aws, backup_root, output):
    # Re-evaluate every eligibility condition immediately before any mutation.
    nodes, volumes = inventory(aws)
    fresh = make_plan(nodes, volumes, [evidence(f['evidence']['root']) for f in plan['fleets']],
                      time.time(), plan['min_age_hours'])
    allowed = {f['run']: f for f in fresh['fleets']}
    for fleet in plan['fleets']:
        need(allowed.get(fleet['run']) == fleet, 'Cloud/evidence changed since planning: ' + fleet['run'])
    receipts = []
    for fleet in plan['fleets']:
        archived = backup(fleet, backup_root)
        nodes, volumes = inventory(aws)
        current = make_plan(nodes, volumes, [evidence(fleet['evidence']['root'])],
                            time.time(), plan['min_age_hours'])
        need(fleet in current['fleets'], 'Fleet changed before termination: ' + fleet['run'])
        receipt = {'run': fleet['run'], 'backup': archived, 'instance_ids': fleet['instance_ids'],
                   'volumes': fleet['volumes'], 'started_at': time.time(), 'state': 'termination_requested'}
        receipts.append(receipt)
        save(Path(output) / 'execution.json', receipts)
        aws('ec2', 'terminate-instances', '--instance-ids', *fleet['instance_ids'])
        end = time.monotonic() + 240
        while True:
            raw = aws('ec2', 'describe-instances', '--instance-ids', *fleet['instance_ids'])
            observed = {n['InstanceId']: n['State']['Name'] for r in raw['Reservations'] for n in r['Instances']}
            if observed == {i: 'terminated' for i in fleet['instance_ids']}:
                break
            need(time.monotonic() < end, 'Termination timeout; inspect execution.json before retrying')
            time.sleep(5)
        for volume in fleet['volumes']:
            # No force detach. Delete only the exact tagged disk after EC2 releases it.
            end = time.monotonic() + 120
            while True:
                rows = aws('ec2', 'describe-volumes', '--filters', 'Name=volume-id,Values=' + volume['id'])['Volumes']
                if not rows:  # DeleteOnTermination may already have released storage.
                    break
                need(len(rows) == 1, 'Ambiguous volume identity')
                v = rows[0]
                need(qualification(tags(v)) and tags(v).get('Run') == fleet['run']
                     and tags(v).get('Role') == volume['role'] and v['Size'] == volume['gib'], 'Volume changed')
                if v['State'] == 'available' and not v['Attachments']:
                    aws('ec2', 'delete-volume', '--volume-id', volume['id'])
                    break
                need(time.monotonic() < end, 'Volume release timeout; inspect execution.json')
                time.sleep(3)
        receipt.update(state='terminated_and_delete_requested', completed_at=time.time())
        save(Path(output) / 'execution.json', receipts)
        print(json.dumps({'retired': fleet['run'], 'volumes': len(fleet['volumes'])}), flush=True)
    remaining = {v['VolumeId'] for v in aws('ec2', 'describe-volumes')['Volumes']}
    expected = {v['id'] for f in plan['fleets'] for v in f['volumes']}
    need(not expected & remaining, 'Some deleted volumes still visible; verify before claiming savings')
    result = {'at': time.time(), 'fleets_removed': len(receipts), 'volumes_removed': len(expected),
              'gib_removed': plan['gib'], 'protected_latest_run': plan['protected_latest_run'],
              'all_target_volumes_absent': True}
    save(Path(output) / 'result.json', result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', required=True)
    parser.add_argument('--region', required=True)
    parser.add_argument('--account-id', required=True)
    parser.add_argument('--state-root', type=Path, action='append', required=True,
                        help='Run directory or a directory of runs; repeat for each location')
    parser.add_argument('--backup-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--min-age-hours', type=float, default=24)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    os.umask(0o077)
    # All invocations on this operator share a lock, including scheduled runs.
    locks = [(root / '.botho-cloud-retention.lock').open('a') for root in sorted(set(args.state_root))]
    for lock in locks:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    aws = Aws(args.profile, args.region)
    need(aws('sts', 'get-caller-identity')['Account'] == args.account_id, 'Wrong AWS account')
    candidates = []
    errors = []
    for root in evidence_roots(args.state_root):
        try:
            candidates.append(evidence(root))
        except (RuntimeError, OSError, ValueError, KeyError, TypeError) as exc:
            errors.append({'root': str(root), 'error': str(exc)})
    nodes, volumes = inventory(aws)
    plan = make_plan(nodes, volumes, candidates, time.time(), args.min_age_hours)
    plan.update(account_id=args.account_id, region=args.region, invalid_archives=errors)
    save(args.output / 'plan.json', plan)
    print(json.dumps({k: plan[k] for k in ('volume_count', 'gib', 'protected_latest_run', 'retained')}), flush=True)
    if args.apply:
        print(json.dumps(apply_plan(plan, aws, args.backup_root, args.output)), flush=True)


if __name__ == '__main__':
    main()
