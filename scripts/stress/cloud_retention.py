#!/usr/bin/env python3
"""Retire archived or verified unused qualification fleets; age alone is insufficient.

Default: write a reviewable plan. --apply also executes it. The newest cloud
fleet is always retained, and automatic use should retain archives for 24h.
Only qualification fleets are eligible: the legacy Project=botho-qualification
tag, or project=botho with Purpose=qualification. No snapshots,
public nodes, build machines, or untagged resources are removed.
An undeployed fleet needs complete remote retirement proof instead of archives.

The plan's volume_count and gib describe disks selected for retirement.
tagged_storage also counts available and in-use qualification disks across
the region, including detached disks and disks on protected fleets.
"""
import argparse
import collections
import datetime
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

PROJECT = 'botho-qualification'
ROLES = {'controller', 'n1', 'n2', 'n3', 'n4', 'n5'}
# EC2 tag filters for each supported scheme; all filters in a row must match.
SCHEMES = (('Name=tag:Project,Values=' + PROJECT,),
           ('Name=tag:project,Values=botho', 'Name=tag:Purpose,Values=qualification'))
DISPOSITIONS = ('held_before_t0', 'aborted_after_deployment', 'lost_after_deployment')


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


def checked_path(root, name):
    """Keep evidence and backup paths inside the selected directory."""
    path = Path(name)
    need(name and not path.is_absolute() and path.as_posix() == name and '..' not in path.parts
         and name != '.', 'Invalid evidence filename')
    target = root / path
    need(target.resolve() == root.resolve() / path, 'Symlink in evidence path: ' + name)
    return target


def timestamp(value):
    return type(value) in (float, int) and math.isfinite(value) and value > 0


def fleet_identities(root):
    inventory = read(checked_path(root, 'inventory.json'))
    identities = {n['id']: n['role'] for n in inventory}
    need(len(inventory) == len(identities) == 6 and set(identities.values()) == ROLES,
         'Expected exactly six distinct roles and instances')
    return identities


def memory_evidence(root):
    """Check what the five collectors exported, including failed qualification runs."""
    detached = (root / 'outcome').exists() or (root / 'final-receipt.json').exists()
    folder = checked_path(root, 'outcome') if detached else root
    index = checked_path(folder, 'SHA256SUMS')
    files = {}
    index_bytes = index.read_bytes()
    for line in index_bytes.decode().splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  (?:\./)?([a-zA-Z0-9_./-]+)', line)
        need(match, 'Invalid memory checksum entry')
        expected, name = match.groups()
        path = checked_path(folder, name)
        need(name not in files and name != 'SHA256SUMS' and path.is_file() and digest(path) == expected,
             'Memory archive checksum mismatch: ' + name)
        files[name] = expected
    if not detached and 'manifest.json' not in files and 'n1-manifest.json' in files:
        canonical = checked_path(folder, 'manifest.json')
        need(digest(canonical) == files['n1-manifest.json'], 'Memory manifest differs from archived collector copy')
        files['manifest.json'] = files['n1-manifest.json']
    need({'manifest.json', 'analysis.json'} <= files.keys(), 'Missing memory manifest or analysis')
    manifest = read(folder / 'manifest.json')
    assessment = read(folder / 'analysis.json')
    run = manifest['campaign_id']
    validators = ROLES - {'controller'}
    manifest_sha = hashlib.sha256(json.dumps(manifest, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()).hexdigest()
    nodes = manifest['nodes']
    need(re.fullmatch(r'q[0-9]{8}-[a-z0-9-]+', run) and timestamp(manifest['started_at_unix_s']) and
         len(nodes) == 5 and {n['node_id'] for n in nodes} == validators and
         len({n['host_id'] for n in nodes}) == 5 and all(n['chain_id'] == run for n in nodes),
         'Memory fleet identity mismatch')
    need(assessment['manifest_sha256'] == manifest_sha and assessment['status'] in ('passed', 'failed', 'incomplete')
         and set(assessment['nodes']) == validators, 'Memory analysis differs from manifest')
    observations = assessment['observation_files']
    observed = {Path(row['path']).name: row['sha256'] for row in observations}
    need(len(observations) == len(observed) == 5, 'Expected five distinct memory observations')
    for role in sorted(validators):
        samples = [name for name in (role + '.jsonl', f'{role}-{role}.jsonl') if name in observed]
        need(len(samples) == 1, 'Missing or duplicate memory observation: ' + role)
        name = samples[0]
        summary = name.removesuffix('.jsonl') + '.summary.json'
        need(name in files and summary in files and files[name] == observed[name], 'Unverified collector: ' + role)
        record = read(folder / summary)
        need(record['nodes'] == [role] and record['manifest_sha256'] == manifest_sha
             and record['out_sha256'] == files[name], 'Memory collector identity mismatch: ' + role)
    files['SHA256SUMS'] = hashlib.sha256(index_bytes).hexdigest()
    archived_at = max(index.stat().st_mtime, manifest['started_at_unix_s'])
    if detached:
        need({p.name for p in folder.iterdir()} == set(files), 'Unindexed detached memory evidence')
        need('result.json' in files, 'Missing terminal memory result')
        receipt = read(checked_path(root, 'final-receipt.json'))
        result = read(folder / 'result.json')
        need(receipt == dict(result, checksum_index_sha256=files['SHA256SUMS']) and result['run'] == run
             and result['commit'] == manifest['source_revision'] and result['artifact_sha256'] == manifest['artifact_sha256']
             and result['status'] in ('passed', 'failed', 'incomplete')
             and timestamp(result['at']) and result['at'] >= manifest['started_at_unix_s'],
             'Memory driver receipt differs from collected result')
        archived_at = max(archived_at, result['at'])
        files = {'outcome/' + name: value for name, value in files.items()}
        files['final-receipt.json'] = digest(root / 'final-receipt.json')
    else:
        # Early collectors indexed the five sample sets before the operator
        # added the host identity and soak receipts to the flat archive.
        for name in ['soak-result.json', 'soak-raw.jsonl', *(f'ident/{role}-identity.json' for role in sorted(validators))]:
            if name not in files and (folder / name).is_file():
                files[name] = digest(checked_path(folder, name))
    return dict(kind='memory', run=run, proof_sha256=files, verified_at=manifest['started_at_unix_s'],
                archived_at=archived_at)


def held_reports(stream, run):
    """Each archived controller attempt must have stopped before its campaign clock started."""
    attempts, reports = set(), {}
    with tarfile.open(fileobj=stream, mode='r|*') as state:
        for entry in state:
            path = Path(entry.name)
            if len(path.parts) < 3 or path.parts[:2] != ('var', 'lib') or not path.parts[2].startswith('botho-controller-'):
                continue
            attempt = path.parts[2].removeprefix('botho-controller-')
            need(attempt == run or re.fullmatch(re.escape(run) + r'-[a-z0-9-]+', attempt), 'Unexpected controller run')
            attempts.add(attempt)
            if path.parts[3:] != ('report.json',):
                continue
            need(entry.isfile() and entry.size <= 32 * 1024 * 1024 and attempt not in reports, 'Invalid terminal report')
            report = json.load(state.extractfile(entry))
            need(report['run_id'] == attempt and report['status'] in ('incomplete', 'held') and report['start'] is None
                 and timestamp(report['setup_start']) and timestamp(report['at']) and report['at'] >= report['setup_start'],
                 'Controller report is not terminal before T0')
            reports[attempt] = report['setup_start']
    need(attempts == reports.keys(), 'No terminal evidence for every controller attempt')
    return min(reports.values(), default=None)


def host_manifest(root):
    manifest = read(checked_path(root, 'final-evidence-manifest.json'))
    need(isinstance(manifest, dict) and set(manifest) == ROLES, 'Incomplete evidence manifest')
    for role, expected in manifest.items():
        path = checked_path(root, f'final-evidence/{role}.tar.gz')
        need(digest(path) == expected, 'Archive checksum mismatch: ' + role)
    return manifest


def archive_hold(root, run):
    verified_at = None
    for role in sorted(ROLES):
        markers = {}
        state_found = False
        with tarfile.open(root / 'final-evidence' / (role + '.tar.gz'), 'r|gz') as archive:
            for entry in archive:
                name = Path(entry.name).as_posix()
                if name in ('markers/run-id', 'markers/role'):
                    need(entry.isfile() and entry.size <= 256 and name not in markers, 'Invalid fleet archive marker')
                    markers[name] = archive.extractfile(entry).read().decode().strip()
                elif name == 'state.tar.gz':
                    need(entry.isfile() and entry.size > 0 and not state_found, 'Invalid archived host state')
                    state_found = True
                    if role == 'controller':
                        verified_at = held_reports(archive.extractfile(entry), run)
        need(state_found and markers == {'markers/run-id': run, 'markers/role': role}, 'Fleet archive marker mismatch: ' + role)
    return verified_at


def recovered_files(root):
    """Files already collected from a fleet with no complete terminal result."""
    patterns = ('outcome/**/*', 'analysis.json', 'manifest.json', 'SHA256SUMS', 'final-receipt.json',
                'n[1-5]*.jsonl', 'n[1-5]-manifest.json', 'n[1-5]*.exit-status', 'n[1-5]*.summary.json',
                'soak-raw.jsonl', 'soak-result.json',
                'ident/*-identity.json')
    if not (root / 'final-evidence-manifest.json').exists():
        patterns += ('final-evidence/*.tar.gz',)
    return {p.relative_to(root).as_posix() for pattern in patterns for p in root.glob(pattern)
            if p.is_file()}


def disposition_proof(root, record, infra, identities):
    run = infra['run_id']
    need(record.get('run') == run and record.get('instances') == identities, 'Disposition fleet identity mismatch')
    need(timestamp(record.get('at')) and record['at'] >= infra['created_at'], 'Invalid disposition time')
    for field in ('decided_by', 'reason', 'disk_contents'):
        need(isinstance(record.get(field), str) and record[field].strip(), 'Missing operator disposition: ' + field)
    files = record['preserved_files']
    need(isinstance(files, dict), 'Invalid disposition file list')
    need(recovered_files(root) <= files.keys(), 'Disposition omits collected evidence')
    proof = {}
    for name, expected in files.items():
        path = checked_path(root, name)
        need(path.is_file() and digest(path) == expected, 'Disposition file checksum mismatch: ' + name)
        proof[name] = expected
    return dict(kind=record['state'], run=run, verified_at=record['at'], proof_sha256=proof)


def record_disposition(root, state, decided_by, reason, disk_contents, preserve):
    """Publish a local operator decision without changing a fleet or an earlier decision."""
    infra = read(checked_path(root, 'infrastructure-plan.json'))
    identities = fleet_identities(root)
    need(state in DISPOSITIONS, 'Unknown operator disposition')
    if state == 'held_before_t0':
        need((root / 'final-evidence-manifest.json').is_file(), 'Missing workload archives for hold before T0')
        host_manifest(root)
        archive_hold(root, infra['run_id'])
    names = set(preserve) | recovered_files(root)
    need(not names.intersection({'retirement.json', 'fleet-final-state.json'}), 'Disposition cannot pin its own decision or subsequent stop record')
    files = {name: digest(checked_path(root, name)) for name in sorted(names)}
    record = dict(state=state, run=infra['run_id'], instances=identities, at=time.time(), decided_by=decided_by,
                  reason=reason, disk_contents=disk_contents, preserved_files=files)
    path = root / 'retirement.json'
    # Validate the operator-supplied fields before publishing a decision.
    disposition_proof(root, record, infra, identities)
    stage = tempfile.NamedTemporaryFile(mode='w', dir=root, delete=False)
    tmp = Path(stage.name)
    try:
        with stage as stream:
            json.dump(record, stream, indent=2)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.link(tmp, path)
        except FileExistsError:
            previous = read(checked_path(root, 'retirement.json'))
            need(previous == dict(record, at=previous.get('at')), 'Different retirement decision already exists')
            disposition_proof(root, previous, infra, identities)
        return path
    finally:
        tmp.unlink(missing_ok=True)


def workload_terminal(root, names, identities, manifest):
    """Tie saved controller and supervisor reports to the fleet and its last observed attempts."""
    files = {}

    def load(name):
        raw = checked_path(root, name).read_bytes()
        files[name] = hashlib.sha256(raw).hexdigest()
        record = json.loads(raw)
        need(isinstance(record, dict), 'Invalid report object: ' + name)
        return record

    infra = load('infrastructure-plan.json')
    run, created = infra['run_id'], infra['created_at']
    need(isinstance(run, str) and re.fullmatch(r'q[0-9]{8}-[a-z0-9-]+', run) and timestamp(created),
         'Invalid terminal report infrastructure plan')
    # The x-series operator appends one attempt letter. Earlier drivers also
    # appended the execution mode to the infrastructure run ID.
    suffix = r'(?:-[a-z])?' if re.match(r'q[0-9]{8}-x[1-9][0-9]*-', run) else r'(?:-(?:[a-z]|campaign|rehearsal))?'
    attempt_pattern = re.compile(re.escape(run) + suffix)
    latest = {}
    held_at = None
    for name in names:
        record = load(name)
        at = record.get('at')
        need(timestamp(at) and at >= created, 'Invalid terminal report time: ' + name)
        if name == 'supervisor-held.json':
            need(record.get('no_restart') is True and record.get('original_shutdown_deadlines_retained') is True
                 and isinstance(record.get('error'), str) and record['error'].strip(), 'Invalid supervisor hold')
            held_at = at
            continue
        attempt = record.get('run_id')
        need(isinstance(attempt, str) and attempt_pattern.fullmatch(attempt)
             and name in ('result.json', 'final-report-' + attempt + '.json'), 'Report fleet or attempt mismatch: ' + name)
        report = record.get('report', record)
        need(isinstance(report, dict) and report.get('run_id') == attempt and timestamp(report.get('at'))
             and created <= report['at'] <= at, 'Invalid controller report identity or time: ' + name)
        status = report.get('status')
        need(isinstance(status, str) and status.strip(), 'Missing controller report status: ' + name)
        if status == 'held_pre_funding':
            need(report.get('rehearsal_started') is False and type(report.get('funding_writes')) is int
                 and report['funding_writes'] == 0, 'Invalid pre-funding hold')
        observed = report['at']
        if attempt in latest and observed == latest[attempt][0]:
            need(status == latest[attempt][1], 'Controllers reported conflicting states at the same time')
        if attempt not in latest or observed > latest[attempt][0]:
            latest[attempt] = (observed, status)
    terminal = {'complete', 'incomplete', 'held', 'rehearsal_complete', 'held_pre_funding'}
    need(all(status in terminal for _, status in latest.values()), 'No terminal controller report for an observed attempt')
    checkpoint_name = '.evidence-collection/checkpoint.json'
    if (root / checkpoint_name).exists():
        checkpoint = load(checkpoint_name)
        selection = checkpoint['selection']
        attempts = selection['attempts']
        need(selection['run'] == run and selection['instances'] == {role: i for i, role in identities.items()}
             and checkpoint['hashes'] == manifest, 'Collection checkpoint differs from the fleet or host archives')
        need(isinstance(attempts, list) and attempts and len(attempts) == len(set(attempts))
             and set(attempts) <= latest.keys(), 'Missing terminal report for a collected attempt')
    verified_at = max([t for t, _ in latest.values()] + ([held_at] if held_at is not None else []))
    for name in ('inventory.json', 'fleet-final-state.json'):
        files[name] = digest(checked_path(root, name))
    # The runner keeps its execution receipts outside the six host archives.
    for name in ('campaign-plan.json', 'campaign-start.json', 'faults.jsonl'):
        if (root / name).exists():
            files[name] = digest(checked_path(root, name))
    return dict(kind='terminal_workload', run=run, verified_at=verified_at, proof_sha256=files,
                archived_at=max(verified_at, *(checked_path(root, name).stat().st_mtime for name in names)))


def evidence(root):
    root = Path(root)
    identities = fleet_identities(root)
    final = read(checked_path(root, 'fleet-final-state.json'))
    need(set(final) == set(identities) and set(final.values()) <= {'stopped', 'terminated'},
         'Recorded fleet not retired')
    manifest_path = root / 'final-evidence-manifest.json'
    retirement_path = root / 'retirement.json'
    retired = read(checked_path(root, 'retirement.json')) if retirement_path.is_file() else {}
    need(isinstance(retired, dict), 'Invalid operator disposition')
    memory = any((root / name).exists() for name in ('analysis.json', 'outcome', 'final-receipt.json'))
    reports = sorted(p.name for p in root.glob('final-report-*.json'))
    reports.extend(name for name in ('result.json', 'supervisor-held.json') if (root / name).is_file())
    extra = {}
    if retired.get('state') in DISPOSITIONS:
        if retired['state'] == 'held_before_t0':
            need(manifest_path.is_file(), 'Missing workload archives for hold before T0')
        extra = disposition_proof(root, retired, read(checked_path(root, 'infrastructure-plan.json')), identities)
        extra['archived_at'] = max(retirement_path.stat().st_mtime, retired['at'])
        for name in ('retirement.json', 'infrastructure-plan.json'):
            extra['proof_sha256'][name] = digest(root / name)
    elif memory:
        extra = memory_evidence(root)
        shutdown = root / 'fleet-shutdown.json'
        if shutdown.is_file():
            recorded = read(checked_path(root, shutdown.name))
            stopped_ids = {v['instance_id']: role for role, v in recorded['instances'].items()}
            need(recorded['run'] == extra['run'] and stopped_ids == identities,
                 'Memory shutdown record differs from fleet inventory')
            extra['proof_sha256'][shutdown.name] = digest(shutdown)
    if extra:
        for name in ('inventory.json', 'fleet-final-state.json'):
            extra['proof_sha256'][name] = digest(root / name)
        if not manifest_path.exists():
            return dict(root=str(root), identities=identities, sha256={}, **extra)
    elif not manifest_path.exists() or (retired.get('state') == 'aborted_before_deployment' and not reports):
        need(retirement_path.is_file(), 'No terminal evidence')
        retirement = retired
        need(retirement.get('state') == 'aborted_before_deployment', 'Missing workload archives')
        infra = read(checked_path(root, 'infrastructure-plan.json'))
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
        extra = dict(unused_verified_at=retirement['at'], run=infra['run_id'],
                     proof_sha256={name: digest(root / name) for name in
                                   ('retirement.json', 'infrastructure-plan.json', 'inventory.json', 'fleet-final-state.json')},
                     archived_at=retirement_path.stat().st_mtime)
        if not manifest_path.exists():
            return dict(root=str(root), identities=identities, sha256={}, **extra)
    manifest = host_manifest(root)
    if not extra and reports:
        extra = workload_terminal(root, reports, identities, manifest)
    elif not extra:
        need(not retirement_path.exists(), 'Unrecognized operator disposition')
        need((root / 'infrastructure-plan.json').is_file(), 'No terminal evidence')
        run = read(checked_path(root, 'infrastructure-plan.json'))['run_id']
        verified_at = archive_hold(root, run)
        extra = dict(kind='held_before_t0' if verified_at else 'no_controller_report', run=run,
                     proof_sha256={name: digest(root / name) for name in ('inventory.json', 'fleet-final-state.json', 'infrastructure-plan.json')})
        if verified_at is not None:
            extra['verified_at'] = verified_at
    elif extra.get('kind') == 'held_before_t0':
        earliest = archive_hold(root, extra['run'])
        if earliest is not None:
            extra['verified_at'] = min(earliest, extra['verified_at'])
    return {**extra, 'root': str(root), 'identities': identities, 'sha256': manifest,
            'archived_at': max(extra.get('archived_at', 0), manifest_path.stat().st_mtime)}


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
            elif e.get('kind') == 'no_controller_report':
                reason = 'Host archives verified; no terminal controller report or operator disposition'
            elif any(e['identities'][n['InstanceId']] != tags(n)['Role'] for n in nodes):
                reason = 'Archive role mismatch'
            elif 'unused_verified_at' in e and (e['run'] != run or any(
                    datetime.datetime.fromisoformat(n['LaunchTime'].replace('Z', '+00:00')).timestamp()
                    > e['unused_verified_at'] for n in nodes)):
                reason = 'Unused-host proof predates last instance launch or has wrong run'
            elif 'verified_at' in e and (e['run'] != run or e['verified_at'] > now or any(
                    datetime.datetime.fromisoformat(n['LaunchTime'].replace('Z', '+00:00')).timestamp()
                    > e['verified_at'] for n in nodes)):
                reason = 'Terminal evidence has a different run or instance lifetime'
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
    allocated = {v['VolumeId']: v for v in volumes
                 if qualification(tags(v)) and v['State'] in ('available', 'in-use')}
    allocated_gib = sum(v['Size'] for v in allocated.values())
    unattached = [v for v in allocated.values() if not v['Attachments']]
    plan['tagged_storage'] = {
        'allocated_volumes': len(allocated), 'allocated_gib': allocated_gib,
        'retained_volumes': len(allocated) - plan['volume_count'], 'retained_gib': allocated_gib - plan['gib'],
        'unattached_volumes': len(unattached), 'unattached_gib': sum(v['Size'] for v in unattached)}
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
                    (root / name).is_file() for name in ('final-evidence-manifest.json', 'retirement.json',
                                                         'analysis.json', 'outcome/analysis.json', 'final-receipt.json'))):
                seen.add(root.resolve())
                yield root


def lock_state_roots(state_roots):
    """Take an exclusive non-blocking lock in each state root and return the open lock files.

    Invocations (including scheduled runs) that name the same state root
    exclude each other; locks are per state root, not per AWS account, so runs
    over disjoint roots do not. Roots are deduped by resolved path, so
    `--state-root r --state-root /abs/r` takes one lock instead of locking
    itself out (#1572).
    """
    locks = []
    try:
        for root in sorted({Path(r).resolve() for r in state_roots}):
            locks.append((root / '.botho-cloud-retention.lock').open('a'))
            fcntl.flock(locks[-1], fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        for lock in locks:
            lock.close()
        raise
    return locks


def backup(fleet, destination):
    original = evidence(fleet['evidence']['root'])
    need(original['sha256'] == fleet['evidence']['sha256'], 'Evidence changed since planning')
    need(original['identities'] == fleet['evidence']['identities'], 'Archive identity changed')
    need(original.get('proof_sha256') == fleet['evidence'].get('proof_sha256'), 'Retirement proof changed')
    source = Path(original['root'])
    dest = Path(destination) / source.name
    need(not dest.resolve().is_relative_to(source.resolve()) and not source.resolve().is_relative_to(dest.resolve()),
         'Backup must be independent of original directory')
    dest.mkdir(parents=True, exist_ok=True, mode=0o700)
    copies = {f'final-evidence/{role}.tar.gz': value for role, value in original['sha256'].items()}
    copies.update(original.get('proof_sha256', {}))
    for name in ('inventory.json', 'fleet-final-state.json', 'final-evidence-manifest.json',
                 'result.json', 'supervisor-held.json', 'retirement.json', 'infrastructure-plan.json',
                 'prefund-manifest.json', 'rehearsal-report.json', 'wallet-backup-receipt.json'):
        if name not in copies and (source / name).is_file():
            copies[name] = digest(checked_path(source, name))
    for name, expected in copies.items():
        original_file = checked_path(source, name)
        target = checked_path(dest, name)
        target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if not target.exists():
            with tempfile.NamedTemporaryFile(dir=target.parent, delete=False) as staged:
                tmp = Path(staged.name)
            try:
                shutil.copyfile(original_file, tmp)
                need(digest(tmp) == expected, 'Backup copy failed checksum')
                try:
                    os.link(tmp, target)
                except FileExistsError:
                    pass  # Verify the copy already published by another process below.
            finally:
                tmp.unlink(missing_ok=True)
        need(not target.samefile(original_file), 'Backup and original share the same file: ' + name)
        need(digest(target) == expected, 'Existing backup checksum mismatch')
    verified = evidence(dest)
    need(verified['identities'] == original['identities'] and verified['sha256'] == original['sha256'],
         'Independent archive verification failed')
    need(all(verified.get(key) == original.get(key) for key in ('kind', 'run', 'verified_at', 'proof_sha256')),
         'Independent retirement proof mismatch')
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


def disposition_cli(argv):
    parser = argparse.ArgumentParser(description=record_disposition.__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--state', required=True, choices=DISPOSITIONS)
    parser.add_argument('--decided-by', required=True)
    parser.add_argument('--reason', required=True)
    parser.add_argument('--disk-contents', required=True)
    parser.add_argument('--preserve', action='append', default=[], metavar='RELATIVE_PATH')
    args = parser.parse_args(argv)
    try:
        print(record_disposition(args.root, args.state, args.decided_by, args.reason, args.disk_contents, args.preserve))
    except (OSError, RuntimeError, ValueError, KeyError, TypeError, EOFError, tarfile.TarError) as exc:
        parser.exit(1, f'{exc}\n')


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
    # Held for the life of the process; see lock_state_roots for the scope.
    locks = lock_state_roots(args.state_root)  # noqa: F841
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    aws = Aws(args.profile, args.region)
    need(aws('sts', 'get-caller-identity')['Account'] == args.account_id, 'Wrong AWS account')
    candidates = []
    errors = []
    for root in evidence_roots(args.state_root):
        try:
            candidates.append(evidence(root))
        except (RuntimeError, OSError, ValueError, KeyError, TypeError, AttributeError, EOFError, tarfile.TarError) as exc:
            errors.append({'root': str(root), 'error': str(exc)})
    nodes, volumes = inventory(aws)
    plan = make_plan(nodes, volumes, candidates, time.time(), args.min_age_hours)
    plan.update(account_id=args.account_id, region=args.region, invalid_archives=errors)
    save(args.output / 'plan.json', plan)
    summary = ('volume_count', 'gib', 'tagged_storage', 'protected_latest_run', 'retained')
    print(json.dumps({k: plan[k] for k in summary}), flush=True)
    if args.apply:
        print(json.dumps(apply_plan(plan, aws, args.backup_root, args.output)), flush=True)


if __name__ == '__main__':
    if sys.argv[1:2] == ['record-disposition']:
        disposition_cli(sys.argv[2:])
    else:
        main()
