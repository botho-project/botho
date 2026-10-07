import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from cloud_retention import PROJECT, ROLES, apply_plan, backup, evidence, make_plan


def tagged(**values):
    return [{'Key': k, 'Value': v} for k, v in values.items()]


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
            self.nodes.append({'InstanceId': identity, 'Tags': tag, 'LaunchTime': '2026-10-01',
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
                           'LaunchTime': '2026-10-02', 'State': {'Name': 'stopped'}, 'BlockDeviceMappings': []})
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
