import copy
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import fleet_provision
from cloud_retention import (PROJECT, ROLES, apply_plan, backup, evidence, evidence_roots, inventory,
                             digest, lock_state_roots, make_plan, record_disposition)


def tagged(**values):
    return [{'Key': k, 'Value': v} for k, v in values.items()]


def tags_of(row):
    return {t['Key']: t['Value'] for t in row['Tags']}


class RetentionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / 'botho-live-test'
        self.root.mkdir()
        (self.root / 'final-evidence').mkdir()
        self.nodes = []
        self.volumes = []
        manifests = {}
        identities = []
        for role in sorted(ROLES):
            identity = 'i-' + role
            volume = 'vol-' + role
            tag = tagged(Project=PROJECT, Run='q20261001-a', Role=role)
            self.nodes.append({'InstanceId': identity, 'Tags': tag, 'LaunchTime': '2026-10-01T00:00:00+00:00',
                               'State': {'Name': 'stopped'}, 'BlockDeviceMappings': [{'Ebs': {'VolumeId': volume}}]})
            self.volumes.append({'VolumeId': volume, 'Size': 40, 'State': 'in-use', 'Tags': tag,
                                 'Attachments': [{'InstanceId': identity}]})
            identities.append({'id': identity, 'role': role})
            raw = ('archive-' + role).encode()
            (self.root / 'final-evidence' / (role + '.tar.gz')).write_bytes(raw)
            manifests[role] = hashlib.sha256(raw).hexdigest()
        self.put('inventory.json', identities)
        self.put('infrastructure-plan.json', {'run_id': 'q20261001-a', 'created_at': 1790812800})
        self.put('supervisor-held.json', {'at': 1790816400, 'error': 'test failed',
                                          'no_restart': True, 'original_shutdown_deadlines_retained': True})
        self.put('fleet-final-state.json', {n['id']: 'stopped' for n in identities})
        self.put('final-evidence-manifest.json', manifests)
        self.e = evidence(self.root)
        self.nodes.append({'InstanceId': 'i-protected', 'Tags': tagged(Project=PROJECT, Run='q20261002-b', Role='controller'),
                           'LaunchTime': '2026-10-02T00:00:00+00:00', 'State': {'Name': 'stopped'}, 'BlockDeviceMappings': []})
        self.now = self.e['archived_at'] + 90000

    def put(self, name, value):
        (self.root / name).write_text(json.dumps(value))

    def plan(self):
        return make_plan(self.nodes, self.volumes, [self.e], self.now, 24)

    def test_only_archived_older_fleet_selected_and_latest_retained(self):
        plan = self.plan()
        self.assertEqual(plan['volume_count'], 6)
        self.assertEqual(plan['gib'], 240)
        self.assertEqual(plan['protected_latest_run'], 'q20261002-b')

    def test_newest_fleet_retained_even_with_valid_archive(self):
        self.nodes.pop()
        plan = self.plan()
        self.assertEqual(plan['volume_count'], 0)
        self.assertEqual(plan['tagged_storage'], {
            'allocated_volumes': 6, 'allocated_gib': 240, 'retained_volumes': 6,
            'retained_gib': 240, 'unattached_volumes': 0, 'unattached_gib': 0})

    def test_storage_report_includes_unattached_qualification_disks(self):
        # A detached volume can outlive its fleet. Its run tag can also be missing.
        orphan = {'VolumeId': 'vol-orphan', 'Size': 80, 'State': 'available', 'Attachments': [],
                  'Tags': tagged(project='botho', Purpose='qualification')}
        self.volumes.extend([orphan, dict(orphan, VolumeId='vol-unrelated', Tags=[]),
                             dict(orphan, VolumeId='vol-deleted', State='deleted')])
        plan = self.plan()
        self.assertEqual((plan['volume_count'], plan['gib']), (6, 240))
        self.assertEqual(plan['tagged_storage'], {
            'allocated_volumes': 7, 'allocated_gib': 320, 'retained_volumes': 1,
            'retained_gib': 80, 'unattached_volumes': 1, 'unattached_gib': 80})
        self.assertEqual([f['run'] for f in plan['fleets']], ['q20261001-a'])

    def retag(self, **values):
        for row in self.nodes[:-1] + self.volumes:
            row['Tags'] = tagged(**values, Run='q20261001-a', Role=tags_of(row)['Role'])

    def test_current_tag_scheme_selected(self):
        self.retag(project='botho', Purpose='qualification', SideProject='botho')
        self.assertEqual(self.plan()['volume_count'], 6)

    def test_x_series_fleet_tagged_by_fleet_provision_selected(self):
        # Ties the tag producer (fleet_provision.tags) to the retention consumer (make_plan).
        self.retag(project='botho', Purpose='qualification', SideProject='botho')
        expected = self.plan()['fleets'][0]
        run = fleet_provision.run_id('20261101', 1, 'endurance')
        for row in self.nodes[:-1] + self.volumes:
            row['Tags'] = fleet_provision.tags(run, tags_of(row)['Role'], 1793786400)
        self.assertEqual(self.plan()['volume_count'], 0)
        self.put('infrastructure-plan.json', {'run_id': run, 'created_at': 1790812800})
        plan = self.refresh()
        self.assertEqual(plan['volume_count'], 6)
        self.assertEqual(plan['protected_latest_run'], 'q20261002-b')
        fleet = plan['fleets'][0]
        self.assertEqual(fleet['run'], 'q20261101-x1-endurance')
        self.assertEqual({k: fleet[k] for k in ('instance_ids', 'volumes')},
                         {k: expected[k] for k in ('instance_ids', 'volumes')})

    def test_public_botho_project_without_qualification_purpose_excluded(self):
        self.retag(project='botho', SideProject='botho')
        plan = self.plan()
        self.assertEqual(plan['volume_count'], 0)
        self.assertEqual(plan['retained'], [{'run': 'q20261002-b', 'reason': 'Newest fleet retained for diagnosis'}])

    def test_newest_fleet_protected_across_tag_schemes(self):
        self.nodes[-1]['Tags'] = tagged(project='botho', Purpose='qualification', Run='q20261002-b', Role='controller')
        self.assertEqual(self.plan()['protected_latest_run'], 'q20261002-b')
        self.assertEqual(self.plan()['volume_count'], 6)

    def test_inventory_merges_both_tag_schemes(self):
        queries = []

        def aws(*args):
            if args[1] == 'describe-volumes':
                return {'Volumes': []}
            queries.append(args[3:])
            return {'Reservations': [{'Instances': [n]} for n in self.nodes[:2]]}

        nodes, _ = inventory(aws)
        self.assertEqual(len(queries), 2)
        self.assertIn(('Name=tag:project,Values=botho', 'Name=tag:Purpose,Values=qualification'), queries)
        self.assertEqual(sorted(n['InstanceId'] for n in nodes), sorted(n['InstanceId'] for n in self.nodes[:2]))

    def test_evidence_roots_span_locations_and_names(self):
        base = Path(self.tmp.name)
        other = base / 'runs'
        other.mkdir()
        run = other / 'live-endurance-w12'
        run.mkdir()
        (run / 'final-evidence-manifest.json').write_text('{}')
        (other / 'live-endurance-w12-archives').mkdir()
        (other / 'alias').symlink_to(self.root)
        found = list(evidence_roots([base, other, run]))
        self.assertEqual(sorted(p.resolve() for p in found), sorted([self.root.resolve(), run.resolve()]))

    def test_state_root_locks_dedupe_by_resolved_path(self):
        # #1572: `--state-root r --state-root /abs/r` used to open two lock
        # files on one path and fail its own second flock.
        base = Path(self.tmp.name)
        (base / 'r').mkdir()
        cwd = os.getcwd()
        os.chdir(base)
        try:
            locks = lock_state_roots([Path('r'), (base / 'r').resolve()])
        finally:
            os.chdir(cwd)
        self.assertEqual(len(locks), 1)
        self.assertEqual(Path(locks[0].name).resolve(), (base / 'r' / '.botho-cloud-retention.lock').resolve())
        # Another invocation over the same root is still excluded.
        with self.assertRaises(BlockingIOError):
            lock_state_roots([base / 'r'])
        for lock in locks:
            lock.close()
        # Disjoint roots do not exclude each other.
        (base / 'other').mkdir()
        held = lock_state_roots([base / 'r'])
        other = lock_state_roots([base / 'other'])
        self.assertEqual(len(held) + len(other), 2)
        for lock in held + other:
            fcntl.flock(lock, fcntl.LOCK_UN)
            lock.close()

    def test_running_node_blocks_entire_fleet(self):
        self.nodes[0]['State']['Name'] = 'running'
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_public_nodes_and_unrelated_disks_never_selected(self):
        self.nodes[0]['Tags'] = tagged(Project='production', Run='q20261001-a', Role='controller')
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_disk_tag_mismatch_blocks_deletion(self):
        self.volumes[0]['Tags'] = tagged(Project=PROJECT, Run='q20261002-b', Role='controller')
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_extra_disk_blocks_entire_fleet(self):
        self.nodes[0]['BlockDeviceMappings'].append({'Ebs': {'VolumeId': 'vol-extra'}})
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_reattached_disk_blocks_deletion(self):
        self.volumes[0]['Attachments'].append({'InstanceId': 'i-other'})
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_role_identity_mismatch_blocks_deletion(self):
        self.e['identities']['i-n1'], self.e['identities']['i-n2'] = 'n2', 'n1'
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_archive_age_gate(self):
        self.now = self.e['archived_at'] + 100
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_archive_corruption_blocks_deletion(self):
        (self.root / 'final-evidence/n3.tar.gz').write_bytes(b'changed')
        with self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
            evidence(self.root)

    def test_missing_archive_blocks_deletion(self):
        (self.root / 'final-evidence/n3.tar.gz').unlink()
        with self.assertRaises(OSError):
            evidence(self.root)

    def test_missing_terminal_evidence_blocks_deletion(self):
        (self.root / 'supervisor-held.json').unlink()
        (self.root / 'infrastructure-plan.json').unlink()
        with self.assertRaisesRegex(RuntimeError, 'terminal'):
            evidence(self.root)

    def test_ongoing_controller_report_does_not_release_disks(self):
        self.infrastructure()
        for status in ('setup', 'running', 'draining', 'unknown'):
            with self.subTest(status=status):
                self.put('result.json', {'run_id': self.run + '-a', 'at': self.started, 'status': status})
                with self.assertRaisesRegex(RuntimeError, 'terminal'):
                    evidence(self.root)

    def test_final_report_supersedes_snapshot_and_survives_backup(self):
        self.infrastructure()
        attempt = self.run + '-a'
        name = 'final-report-' + attempt + '.json'
        first = {'run_id': attempt, 'at': self.started, 'status': 'running'}
        final = dict(first, at=self.started + 10, status='complete')
        self.put('result.json', first)
        self.put(name, final)
        plan = self.refresh()
        dest = Path(backup(plan['fleets'][0], Path(self.tmp.name) / 'secondary'))
        self.assertEqual(json.loads((dest / name).read_bytes()), final)
        self.assertEqual(json.loads((dest / 'result.json').read_bytes()), first)
        self.assertEqual(evidence(dest)['verified_at'], final['at'])
        self.put('result.json', dict(first, at=final['at'] + 1))
        with self.assertRaisesRegex(RuntimeError, 'terminal'):
            evidence(self.root)
        self.put('result.json', dict(first, at=final['at']))
        with self.assertRaisesRegex(RuntimeError, 'conflicting'):
            evidence(self.root)
        self.put('result.json', dict(first, run_id=self.run + '-b'))
        with self.assertRaisesRegex(RuntimeError, 'terminal'):
            evidence(self.root)

    def test_terminal_proof_matches_fleet_attempt_and_time(self):
        self.infrastructure()
        name = 'final-report-' + self.run + '-a.json'
        row = {'run_id': self.run + '-a', 'at': self.started, 'status': 'incomplete'}
        self.put(name, row)
        self.assertEqual(evidence(self.root)['verified_at'], self.started)
        for bad in (dict(row, run_id=self.run + '-b'), dict(row, run_id='q20261009-other-a'),
                    dict(row, at=self.started - 4000), dict(row, at=True)):
            with self.subTest(bad=bad):
                self.put(name, bad)
                with self.assertRaises(RuntimeError):
                    evidence(self.root)

    def test_changed_terminal_report_invalidates_an_existing_plan(self):
        self.infrastructure()
        report = {'run_id': self.run + '-a', 'at': self.started, 'status': 'complete'}
        self.put('result.json', report)
        plan = self.refresh()
        self.put('result.json', dict(report, status='incomplete'))
        with patch('cloud_retention.inventory', return_value=(self.nodes, self.volumes)), \
                patch('cloud_retention.time.time', return_value=self.now):
            with self.assertRaisesRegex(RuntimeError, 'changed since planning'):
                apply_plan(plan, lambda *a: self.fail('Unexpected provider call'), Path(self.tmp.name) / 'secondary', self.tmp.name)

    def test_pre_funding_hold_requires_zero_writes_and_an_unstarted_rehearsal(self):
        self.infrastructure()
        report = {'run_id': self.run, 'at': self.started, 'status': 'held_pre_funding',
                  'funding_writes': 0, 'rehearsal_started': False}
        self.put('result.json', report)
        self.assertEqual(self.refresh()['volume_count'], 6)
        for change in ({'funding_writes': 1}, {'funding_writes': False}, {'rehearsal_started': True}):
            with self.subTest(change=change):
                self.put('result.json', dict(report, **change))
                with self.assertRaisesRegex(RuntimeError, 'pre-funding'):
                    evidence(self.root)

    def test_backup_preserves_the_campaign_plan_and_operator_execution_journal(self):
        self.infrastructure()
        self.put('result.json', {'run_id': self.run + '-a', 'at': self.started, 'status': 'incomplete'})
        self.put('campaign-plan.json', {'profile': 'test'})
        self.put('campaign-start.json', {'start': self.started, 'run_id': self.run + '-a'})
        log = self.root / 'faults.jsonl'
        log.write_text('{"step":"D4","phase":"verify","ok":true}\n')
        fleet = self.refresh()['fleets'][0]
        destination = Path(self.tmp.name) / 'secondary'
        retained = Path(backup(fleet, destination))
        for name in ('faults.jsonl', 'campaign-plan.json', 'campaign-start.json'):
            self.assertEqual((retained / name).read_bytes(), (self.root / name).read_bytes())
        log.write_text('')
        with self.assertRaisesRegex(RuntimeError, 'proof changed'):
            backup(fleet, destination)

    def test_historical_supervisor_and_nested_reports(self):
        self.infrastructure()
        attempt = self.run + '-campaign'
        report = {'run_id': attempt, 'status': 'incomplete', 'at': self.started}
        self.put('result.json', {'run_id': attempt, 'report': report, 'at': self.started + 1})
        self.assertEqual(self.refresh()['volume_count'], 6)
        self.put('result.json', {'run_id': self.run, 'report': report, 'at': self.started + 1})
        with self.assertRaises(RuntimeError):
            evidence(self.root)
        self.put('result.json', dict(report, status='setup'))
        held = {'at': self.started + 5, 'error': 'stopped', 'no_restart': True,
                'original_shutdown_deadlines_retained': True}
        self.put('supervisor-held.json', held)
        with self.assertRaisesRegex(RuntimeError, 'terminal'):
            evidence(self.root)
        (self.root / 'result.json').unlink()
        self.assertEqual(self.refresh()['volume_count'], 6)
        for change in ({'no_restart': False}, {'at': 0}, {'error': ''}):
            with self.subTest(change=change):
                self.put('supervisor-held.json', dict(held, **change))
                with self.assertRaises(RuntimeError):
                    evidence(self.root)

    def test_collection_checkpoint_binds_every_attempt_to_the_host_archives(self):
        self.infrastructure()
        attempt = self.run + '-a'
        self.put('result.json', {'run_id': attempt, 'at': self.started, 'status': 'rehearsal_complete'})
        (self.root / '.evidence-collection').mkdir()
        name = '.evidence-collection/checkpoint.json'
        inventory = json.loads((self.root / 'inventory.json').read_bytes())
        selection = {'run': self.run, 'instances': {n['role']: n['id'] for n in inventory}, 'attempts': [attempt]}
        manifest = json.loads((self.root / 'final-evidence-manifest.json').read_bytes())
        self.put(name, {'selection': selection, 'hashes': manifest})
        plan = self.refresh()
        dest = Path(backup(plan['fleets'][0], Path(self.tmp.name) / 'secondary'))
        self.assertEqual((dest / name).read_bytes(), (self.root / name).read_bytes())
        for updated, hashes in ((dict(selection, attempts=[attempt, self.run + '-b']), manifest),
                                (dict(selection, instances={}), manifest), (selection, {})):
            with self.subTest(updated=updated, hashes=hashes):
                self.put(name, {'selection': updated, 'hashes': hashes})
                with self.assertRaises(RuntimeError):
                    evidence(self.root)

    def test_backup_verified_and_existing_corruption_not_overwritten(self):
        fleet = self.plan()['fleets'][0]
        dest = Path(self.tmp.name) / 'backup'
        result = Path(backup(fleet, dest))
        self.assertEqual(evidence(result)['sha256'], self.e['sha256'])
        (result / 'final-evidence/n1.tar.gz').write_bytes(b'corrupt')
        with self.assertRaisesRegex(RuntimeError, 'backup checksum'):
            backup(fleet, dest)

    def unused(self):
        (self.root / 'final-evidence-manifest.json').unlink()
        self.put('infrastructure-plan.json', {'run_id': 'q20261001-a', 'created_at': 1790812800})
        self.put('retirement.json', {'state': 'aborted_before_deployment', 'at': 1790816400,
                 'remote_verification': {role: {'run': 'q20261001-a', 'bootstrap': True,
                                               'node_unit': False, 'controller_data': False} for role in ROLES}})
        self.e = evidence(self.root)
        self.now = self.e['archived_at'] + 90000

    def test_unused_bootstrap_fleet_can_retire_with_backed_up_proof(self):
        self.unused()
        self.assertEqual(self.plan()['volume_count'], 6)
        dest = backup(self.plan()['fleets'][0], Path(self.tmp.name) / 'unused-backup')
        self.assertEqual(evidence(dest)['proof_sha256'], self.e['proof_sha256'])

    def test_unused_claim_cannot_override_deployment_marker(self):
        self.unused()
        (self.root / 'node-start.intent').write_text('{}')
        with self.assertRaisesRegex(RuntimeError, 'Deployment evidence'):
            evidence(self.root)

    def test_unused_claim_requires_remote_checks_for_all_roles(self):
        self.unused()
        raw = json.loads((self.root / 'retirement.json').read_text())
        raw['remote_verification']['n3']['node_unit'] = True
        self.put('retirement.json', raw)
        with self.assertRaisesRegex(RuntimeError, 'Unused-host verification failed'):
            evidence(self.root)

    def test_unused_claim_invalid_after_instance_relaunch(self):
        self.unused()
        self.nodes[0]['LaunchTime'] = '2026-10-01T02:00:00+00:00'
        self.assertEqual(self.plan()['volume_count'], 0)

    def infrastructure(self):
        self.run = 'q20261001-a'
        self.started = 1790816400
        self.put('infrastructure-plan.json', {'run_id': self.run, 'created_at': self.started - 3600})
        (self.root / 'supervisor-held.json').unlink()

    @staticmethod
    def tar(members):
        content = io.BytesIO()
        with tarfile.open(fileobj=content, mode='w:gz') as archive:
            for name, raw in members.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(raw)
                archive.addfile(entry, io.BytesIO(raw))
        return content.getvalue()

    def host_archives(self, attempts, wrong_marker=None):
        state = {}
        for suffix, properties in attempts.items():
            attempt = self.run + suffix
            prefix = f'var/lib/botho-controller-{attempt}/'
            state[prefix + 'active-plan.json'] = b'{}'
            if properties is not None:
                report = dict(run_id=attempt, status='incomplete', setup_start=self.started, start=None,
                              at=self.started + 120)
                report.update(properties)
                state[prefix + 'report.json'] = json.dumps(report).encode()
        manifest = {}
        for role in ROLES:
            members = {'./markers/role': (wrong_marker if role == 'n3' and wrong_marker else role).encode(),
                       './markers/run-id': self.run.encode(),
                       './state.tar.gz': self.tar(state if role == 'controller' else {})}
            path = self.root / 'final-evidence' / (role + '.tar.gz')
            path.write_bytes(self.tar(members))
            manifest[role] = digest(path)
        self.put('final-evidence-manifest.json', manifest)

    def refresh(self):
        self.e = evidence(self.root)
        self.now = max(self.e['archived_at'], self.e.get('verified_at', 0)) + 90000
        return self.plan()

    def test_all_archived_attempts_held_before_t0(self):
        self.infrastructure()
        self.host_archives({'-a': {'status': 'held'}, '-b': {}})
        plan = self.refresh()
        self.assertEqual(self.e['kind'], 'held_before_t0')
        self.assertEqual(plan['volume_count'], 6)
        dest = backup(plan['fleets'][0], Path(self.tmp.name) / 'secondary')
        self.assertEqual(evidence(dest)['proof_sha256'], self.e['proof_sha256'])
        self.nodes[1]['LaunchTime'] = '2026-10-01T03:00:00Z'
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_open_foreign_or_started_attempt_defeats_hold(self):
        self.infrastructure()
        for properties in (None, {'start': self.started}, {'run_id': 'other'}, {'status': 'running'}):
            with self.subTest(properties=properties):
                self.host_archives({'-a': {}, '-b': properties})
                with self.assertRaises(RuntimeError):
                    evidence(self.root)

    def test_cannot_publish_hold_after_started_campaign(self):
        self.infrastructure()
        self.host_archives({'-a': {'start': self.started}})
        with self.assertRaisesRegex(RuntimeError, 'before T0'):
            self.record('held_before_t0')
        self.assertFalse((self.root / 'retirement.json').exists())

    def test_archive_marker_must_match_role(self):
        self.infrastructure()
        self.host_archives({'-a': {}}, wrong_marker='n2')
        with self.assertRaisesRegex(RuntimeError, 'marker mismatch'):
            evidence(self.root)

    def test_unstaged_without_report_needs_operator_decision(self):
        self.infrastructure()
        self.host_archives({})
        plan = self.refresh()
        self.assertEqual(plan['volume_count'], 0)
        self.assertIn('no terminal controller report', plan['retained'][0]['reason'])
        self.put('retirement.json', {'state': 'unknown'})
        with self.assertRaisesRegex(RuntimeError, 'Unrecognized operator disposition'):
            evidence(self.root)
        (self.root / 'retirement.json').unlink()
        self.record('held_before_t0')
        self.assertEqual(self.refresh()['volume_count'], 6)
        self.host_archives({'-a': {'start': self.started}})
        with self.assertRaisesRegex(RuntimeError, 'before T0'):
            evidence(self.root)

    def memory_files(self, detached=True, status='passed', manifest_alias=False):
        self.infrastructure()
        (self.root / 'final-evidence-manifest.json').unlink()
        folder = self.root / 'outcome' if detached else self.root
        folder.mkdir(exist_ok=True)
        def write(name, value):
            (folder / name).write_text(json.dumps(value))
        roles = sorted(ROLES - {'controller'})
        manifest = dict(campaign_id=self.run, started_at_unix_s=self.started, source_revision='revision',
                        artifact_sha256='artifact', nodes=[dict(node_id=r, chain_id=self.run, host_id='host-' + r) for r in roles])
        manifest_hash = hashlib.sha256(json.dumps(manifest, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        observations = []
        names = []
        for role in roles:
            name = f'{role}-{role}.jsonl'
            summary = f'{role}-{role}.summary.json'
            write(name, {'node_id': role})
            raw_hash = digest(folder / name)
            write(summary, dict(nodes=[role], manifest_sha256=manifest_hash, out_sha256=raw_hash))
            observations.append(dict(path='/collector/' + name, sha256=raw_hash))
            names.extend((name, summary))
        write('manifest.json', manifest)
        manifest_name = 'n1-manifest.json' if manifest_alias else 'manifest.json'
        if manifest_alias:
            write(manifest_name, manifest)
        write('analysis.json', dict(manifest_sha256=manifest_hash, status=status,
                                    nodes={role: {} for role in roles}, observation_files=observations))
        names.extend((manifest_name, 'analysis.json'))
        if detached:
            result = dict(run=self.run, commit='revision', artifact_sha256='artifact', status=status, at=self.started + 50)
            write('result.json', result)
            names.append('result.json')
        index = folder / 'SHA256SUMS'
        index.write_text(''.join(f'{digest(folder / name)}  {name}\n' for name in names))
        if detached:
            self.put('final-receipt.json', dict(result, checksum_index_sha256=digest(index)))
        return folder

    def test_detached_memory_eligible_after_age_and_fully_backed_up(self):
        folder = self.memory_files()
        self.assertIn(self.root, list(evidence_roots([Path(self.tmp.name)])))
        plan = self.refresh()
        self.assertEqual(self.e['kind'], 'memory')
        self.assertEqual(plan['volume_count'], 6)
        destination = Path(self.tmp.name) / 'secondary'
        retained = Path(backup(plan['fleets'][0], destination))
        self.assertEqual(digest(retained / 'outcome/n3-n3.jsonl'), digest(folder / 'n3-n3.jsonl'))
        (retained / 'outcome/n3-n3.jsonl').write_bytes(b'corrupt')
        with patch('cloud_retention.inventory', return_value=(self.nodes, self.volumes)), patch('cloud_retention.time.time', return_value=self.now):
            with self.assertRaisesRegex(RuntimeError, 'Existing backup checksum'):
                apply_plan(plan, lambda *args: self.fail('Unexpected cloud mutation'), destination, self.tmp.name)
        self.now = self.e['archived_at'] + 100
        self.assertEqual(self.plan()['volume_count'], 0)

    def test_hardlinked_memory_log_is_not_an_independent_backup(self):
        folder = self.memory_files()
        plan = self.refresh()
        destination = Path(self.tmp.name) / 'secondary'
        target = destination / self.root.name / 'outcome'
        target.mkdir(parents=True)
        os.link(folder / 'n1-n1.jsonl', target / 'n1-n1.jsonl')
        with self.assertRaisesRegex(RuntimeError, 'share the same file'):
            backup(plan['fleets'][0], destination)

    def test_flat_incomplete_memory_with_collector_manifest(self):
        self.memory_files(detached=False, status='incomplete', manifest_alias=True)
        plan = self.refresh()
        self.assertEqual(plan['volume_count'], 6)
        dest = backup(plan['fleets'][0], Path(self.tmp.name) / 'secondary')
        self.assertEqual(evidence(dest)['proof_sha256'], self.e['proof_sha256'])
        (self.root / 'manifest.json').write_bytes(b'{}')
        with self.assertRaisesRegex(RuntimeError, 'collector copy'):
            evidence(self.root)

    def test_memory_rejects_cross_run_receipt_and_unindexed_file(self):
        folder = self.memory_files()
        saved = json.loads((self.root / 'final-receipt.json').read_text())
        self.put('final-receipt.json', dict(saved, run='q20261002-b'))
        with self.assertRaisesRegex(RuntimeError, 'driver receipt'):
            evidence(self.root)
        self.put('final-receipt.json', saved)
        (folder / 'extra').touch()
        with self.assertRaisesRegex(RuntimeError, 'Unindexed'):
            evidence(self.root)

    def test_memory_rejects_missing_role_even_with_rehashed_index(self):
        folder = self.memory_files(detached=False)
        index = folder / 'SHA256SUMS'
        index.write_text(''.join(line for line in index.read_text().splitlines(True) if 'n3-n3.jsonl' not in line))
        with self.assertRaisesRegex(RuntimeError, 'Unverified collector'):
            evidence(self.root)

    def test_memory_rejects_paths_outside_evidence(self):
        folder = self.memory_files(detached=False)
        index = folder / 'SHA256SUMS'
        external = Path(self.tmp.name) / 'external'
        external.write_text('sample')
        (folder / 'alias').symlink_to(external)
        for name in ('../external', str(external), 'alias'):
            with self.subTest(name=name):
                index.write_text(f'{digest(external)}  {name}\n')
                with self.assertRaises(RuntimeError):
                    evidence(self.root)

    def record(self, state='lost_after_deployment', reason='Fixture controller stopped during funding'):
        return record_disposition(self.root, state, 'fixture operator', reason,
                                  'Fixture node journals and partial output', [])

    def test_local_disposition_cli_idempotent_and_preserves_recovered_files(self):
        self.infrastructure()
        (self.root / 'final-evidence-manifest.json').unlink()
        (self.root / 'outcome').mkdir()
        (self.root / 'outcome/n1-n1.jsonl').write_text('partial')
        cli = Path(__file__).with_name('cloud_retention.py')
        args = [sys.executable, str(cli), 'record-disposition', '--root', str(self.root),
                '--state', 'aborted_after_deployment', '--decided-by', 'fixture operator',
                '--reason', 'Fixture boot failed', '--disk-contents', 'Partial collector and six host archives']
        subprocess.run(args, check=True, capture_output=True)
        path = self.root / 'retirement.json'
        first = path.read_bytes()
        subprocess.run(args, check=True, capture_output=True)
        self.assertEqual(first, path.read_bytes())
        self.assertNotEqual(subprocess.run([*args[:-1], 'Conflicting inventory'], capture_output=True).returncode, 0)
        plan = self.refresh()
        self.assertEqual(plan['volume_count'], 6)
        self.assertEqual(self.e['kind'], 'aborted_after_deployment')
        dest = Path(backup(plan['fleets'][0], Path(self.tmp.name) / 'secondary'))
        self.assertEqual((dest / 'outcome/n1-n1.jsonl').read_text(), 'partial')
        self.assertEqual(digest(dest / 'final-evidence/n5.tar.gz'), digest(self.root / 'final-evidence/n5.tar.gz'))
        (self.root / 'outcome/new-recovered-log').touch()
        with self.assertRaisesRegex(RuntimeError, 'omits collected evidence'):
            evidence(self.root)

    def test_loss_requires_identity_decision_and_preserved_evidence(self):
        self.infrastructure()
        (self.root / 'final-evidence-manifest.json').unlink()
        with self.assertRaisesRegex(RuntimeError, 'Missing operator disposition'):
            self.record(reason=' ')
        self.assertFalse((self.root / 'retirement.json').exists())
        self.record()
        plan = self.refresh()
        self.assertEqual(plan['fleets'][0]['evidence']['kind'], 'lost_after_deployment')
        raw = json.loads((self.root / 'retirement.json').read_text())
        for changes in ({'instances': {}}, {'run': 'different'}, {'at': 1}, {'decided_by': ''}, {'disk_contents': ''}):
            with self.subTest(changes=changes):
                self.put('retirement.json', dict(raw, **changes))
                with self.assertRaises(RuntimeError):
                    evidence(self.root)
        self.put('retirement.json', raw)
        (self.root / 'final-evidence/n5.tar.gz').write_bytes(b'different')
        with self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
            evidence(self.root)

    def test_cloud_change_before_apply_never_terminates(self):
        plan = self.plan()
        self.nodes[0]['State']['Name'] = 'running'
        with patch('cloud_retention.inventory', return_value=(self.nodes, self.volumes)), patch('cloud_retention.time.time', return_value=self.now):
            with self.assertRaisesRegex(RuntimeError, 'changed since planning'):
                apply_plan(plan, lambda *a: self.fail('Unexpected cloud mutation'), Path(self.tmp.name) / 'backup', self.tmp.name)

    def test_cloud_change_during_backup_never_terminates(self):
        plan = self.plan()
        changed = copy.deepcopy(self.nodes)
        changed[0]['State']['Name'] = 'running'
        with patch('cloud_retention.inventory', side_effect=[(self.nodes, self.volumes), (changed, self.volumes)]), patch('cloud_retention.time.time', return_value=self.now):
            with self.assertRaisesRegex(RuntimeError, 'changed before termination'):
                apply_plan(plan, lambda *a: self.fail('Unexpected cloud mutation'), Path(self.tmp.name) / 'backup', self.tmp.name)

    def test_apply_terminates_before_deleting_and_verifies_absence(self):
        plan = self.plan()
        calls = []
        remaining = {v['VolumeId']: copy.deepcopy(v) for v in self.volumes}

        def aws(*args):
            calls.append(args)
            if args[1] == 'terminate-instances':
                return {}
            if args[1] == 'describe-instances':
                return {'Reservations': [{'Instances': [{'InstanceId': n['InstanceId'], 'State': {'Name': 'terminated'}} for n in self.nodes[:-1]]}]}
            if args[1] == 'describe-volumes':
                if len(args) == 2:
                    return {'Volumes': list(remaining.values())}
                identity = args[-1].split('Values=')[1]
                v = remaining[identity]
                v.update(State='available', Attachments=[])
                return {'Volumes': [v]}
            if args[1] == 'delete-volume':
                del remaining[args[-1]]
                return {}
            self.fail(args)

        with patch('cloud_retention.inventory', return_value=(self.nodes, self.volumes)), patch('cloud_retention.time.time', return_value=self.now):
            result = apply_plan(plan, aws, Path(self.tmp.name) / 'backup', self.tmp.name)
        self.assertEqual(result['volumes_removed'], 6)
        self.assertEqual(calls[0][1], 'terminate-instances')
        self.assertEqual(sum(c[1] == 'delete-volume' for c in calls), 6)


if __name__ == '__main__':
    unittest.main()
