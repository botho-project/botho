import json
import tempfile
import unittest
from pathlib import Path
from evidence import inventory,manifest


class EvidenceTests(unittest.TestCase):
    def test_empty_and_skipped_are_not_success(self):
        for outcome in ['success','skipped','failure','cancelled','']:
            self.assertEqual(manifest({'quick':outcome},[])['status'],'missing_measurements')
        self.assertEqual(manifest({'quick':''},[])['step_outcomes']['quick'],'unknown')

    def test_current_files_and_tolerated_failure(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);new=root/'MLSAG_verify/ring_size/16/new';new.mkdir(parents=True)
            for name in ['estimates.json','sample.json']:(new/name).write_text('{}')
            files=inventory(root)
            result=manifest({'clear_reports':'success','quick':'success','full':'skipped'},files)
            self.assertTrue(result['status'].startswith('successful_steps'))
            self.assertEqual(result['current_estimate_files'],1)
            self.assertEqual(result['current_sample_files'],1)
            self.assertEqual(inventory(root),files)
            self.assertEqual(manifest({'clear_reports':'success'},files)['status'],
                             'partial_or_unsuccessful_measurements')
            self.assertEqual(manifest({'quick':'success'},files)['status'],
                             'partial_or_unsuccessful_measurements')
            for outcome in ['failure','cancelled','unknown']:
                self.assertEqual(manifest({'quick':outcome,'full':'skipped'},files)['status'],
                                 'partial_or_unsuccessful_measurements')
            (new/'sample.json').write_text('broken')
            self.assertEqual(manifest({'clear_reports':'success','quick':'success'},inventory(root))['status'],
                             'partial_or_unsuccessful_measurements')

    def test_cached_base_is_not_new_measurement(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d)/'group/base';p.mkdir(parents=True)
            (p/'estimates.json').write_text('{}')
            self.assertEqual(manifest({'clear_reports':'success','quick':'success'},inventory(Path(d)))['status'],
                             'incomplete_criterion_files')

    def test_split_directories_are_not_complete_pairs(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            for group,name in [('a','estimates.json'),('b','sample.json')]:
                p=root/group/'new';p.mkdir(parents=True)
                (p/name).write_text('{}')
            result=manifest({'clear_reports':'success','quick':'success'},inventory(root))
            self.assertEqual(result['status'],'incomplete_criterion_files')
            self.assertEqual(result['matched_current_pairs'],0)

    def test_invalid_outcomes_and_symlink(self):
        for value in [{},{'x':'green'},[]]:
            with self.assertRaises(ValueError):manifest(value,[])
        with tempfile.TemporaryDirectory() as d:
            p=Path(d)/'test.json';p.symlink_to(Path(d)/'absent')
            with self.assertRaises(ValueError):inventory(Path(d))


if __name__=='__main__':unittest.main()
