import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import fleet_provision
from cloud_retention import PROJECT, ROLES, apply_plan, backup, evidence, evidence_roots, inventory, make_plan


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
        self.put('supervisor-held.json', {'error': 'test failed'})
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
        self.assertEqual(self.plan()['volume_count'], 0)

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
        plan = self.plan()
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
        with self.assertRaisesRegex(RuntimeError, 'terminal'):
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
