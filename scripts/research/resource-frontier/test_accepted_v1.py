"""Synthetic importer tests; none of these records is measured evidence."""
import copy
import json
import tempfile
import unittest
from pathlib import Path
from accepted_v1 import SOURCES, UNITS, TEST, COMPILE_COMMAND, digest, load_evidence, observed_inputs, validate_sample


def sample(n):
    files = {k: {'logical_bytes': 4096, 'allocated_bytes': 4096} for k in ('data.mdb', 'lock.mdb')}
    records = {'scope': 'selected_blocks_and_utxos_only', 'rows': 2, 'encoded_key_value_bytes': 100, 'rows_sha256': '0'*64}
    return {'schema': 1, 'kind': 'ordinary_v1_accepted_block_apply', 'status': 'accepted_reopened', 'pid': 1,
            'payments': n, 'transactions': n, 'recipient_outputs': n, 'change_outputs': n,
            'coinbase_outputs': 1, 'lottery_outputs': 0, 'ring_size': 20, 'inputs_per_transaction': 1,
            'units': UNITS.copy(), 'setup': {'wall_ns': 10, 'user_cpu_us': 0, 'system_cpu_us': 0},
            'apply': {'wall_ns': 10, 'user_cpu_us': 0, 'system_cpu_us': 0},
            'objects': {'block_bytes': 100+n*50, 'transaction_bytes': [50]*n, 'block_sha256': '0'*64},
            'storage': {'before_files': copy.deepcopy(files), 'after_files': copy.deepcopy(files),
                        'before_selected_records': records.copy(), 'after_selected_records': records | {'rows': 4+2*n},
                        'selected_block_height': 21, 'selected_utxo_ids': [f'{i:072x}' for i in range(3+2*n)]}}


class EvidenceTests(unittest.TestCase):
    def test_zero_resolution_is_preserved_not_free_network_cost(self):
        result = observed_inputs([validate_sample(sample(0)), validate_sample(sample(1))])
        self.assertIsNone(result['observations'][0]['cpu_us_per_payment'])
        self.assertIsNone(result['electricity_kwh'])
        self.assertEqual(result['observations'][1]['apply_cpu_us'], 0)

    def test_reject_units_fields_denominators_scope(self):
        for mutate in (
            lambda s: s['units'].update(cpu='wall_seconds'),
            lambda s: s['apply'].update(cpu_seconds=1),
            lambda s: s.update(transactions=2),
            lambda s: s['objects'].update(transaction_bytes=[]),
            lambda s: s['storage']['after_selected_records'].update(scope='entire_database'),
            lambda s: s.update(kind='partial_ct_proof'),
            lambda s: s['apply'].update(user_cpu_us=-1),
            lambda s: s.update(schema=True),
            lambda s: s.update(coinbase_outputs=True),
            lambda s: s.update(inputs_per_transaction=True),
            lambda s: s['objects'].update(transaction_bytes=[0]),
            lambda s: s['storage']['selected_utxo_ids'].append(s['storage']['selected_utxo_ids'][0]),
            lambda s: s['storage']['selected_utxo_ids'].__setitem__(0, 'AA'*36),
            lambda s: s['storage']['selected_utxo_ids'].__setitem__(0, '00'*35),
            lambda s: s['storage']['after_selected_records'].update(rows=2),
            lambda s: s['storage']['before_selected_records'].update(rows=3),
        ):
            s = sample(1)
            mutate(s)
            with self.assertRaises(ValueError):
                validate_sample(s)

    def fixture(self, root):
        for path in SOURCES:
            file = root / path
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text('synthetic source for importer testing')
        out = root / 'evidence'
        out.mkdir()
        artifact = {'reason': 'compiler-artifact',
                    'target': {'name': 'tx_lifecycle_integration', 'kind': ['test'], 'crate_types': ['bin'],
                               'src_path': str(root/'botho/tests/tx_lifecycle_integration.rs')},
                    'profile': {'test': True, 'opt_level': '3', 'debug_assertions': False, 'overflow_checks': False},
                    'executable': '/synthetic/tx_lifecycle_integration'}
        (out/'cargo.jsonl').write_text(json.dumps(artifact)+'\n'+json.dumps({'reason': 'build-finished', 'success': True})+'\n')
        m = {'compile_command': COMPILE_COMMAND, 'artifact': artifact, 'executable_sha256': '0'*64,
             'cargo_json_sha256': digest(out/'cargo.jsonl'), 'status': 'complete', 'compile_exit': 0, 'sources': {p: digest(root/p) for p in SOURCES}, 'runs': []}
        for n in range(3):
            for i in range(2):
                case = f'payments-{n}-sample-{i}'
                log = out / (case + '.log')
                log.write_text('V1_RESOURCE_SAMPLE ' + json.dumps(sample(n)) + '\n')
                m['runs'].append({'command': [artifact['executable'], TEST, '--exact', '--ignored', '--nocapture'], 'case': case, 'exit': 0, 'log': log.name, 'sha256': digest(log)})
        (out/'manifest.json').write_text(json.dumps(m))
        return out, m

    def test_complete_matrix_and_source_raw_integrity(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            out, m = self.fixture(root)
            self.assertEqual(len(load_evidence(out, root)[1]), 6)
            log = out / m['runs'][0]['log']
            log.write_text(log.read_text() + 'changed\n')
            with self.assertRaisesRegex(ValueError, 'raw log'):
                load_evidence(out, root)
            m['runs'][0]['sha256'] = digest(log)
            (out/'manifest.json').write_text(json.dumps(m))
            (root/SOURCES[0]).write_text('changed')
            with self.assertRaisesRegex(ValueError, 'source mismatch'):
                load_evidence(out, root)

    def test_cargo_provenance_must_be_consumed(self):
        for change in ('hash', 'duplicate', 'target', 'profile', 'executable', 'manifest', 'command'):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                out, m = self.fixture(root)
                cargo = out/'cargo.jsonl'
                messages = [json.loads(line) for line in cargo.read_text().splitlines()]
                if change == 'duplicate':
                    messages.insert(0, copy.deepcopy(messages[0]))
                elif change == 'target':
                    messages[0]['target']['kind'] = ['lib']
                elif change == 'profile':
                    messages[0]['profile']['test'] = False
                elif change == 'executable':
                    messages[0]['executable'] = 'relative/path'
                elif change == 'manifest':
                    m['artifact']['profile']['opt_level'] = '0'
                elif change == 'command':
                    m['runs'][0]['command'][0] = '/unrelated/executable'
                cargo.write_text(''.join(json.dumps(x)+'\n' for x in messages))
                if change != 'hash':
                    m['cargo_json_sha256'] = digest(cargo)
                else:
                    cargo.write_text(cargo.read_text() + '\n')
                (out/'manifest.json').write_text(json.dumps(m))
                with self.assertRaises(ValueError):
                    load_evidence(out, root)

    def test_exact_half_cpu_without_float_roundtrip(self):
        s = sample(2)
        s['apply']['user_cpu_us'] = 9007199254740993
        result = observed_inputs([validate_sample(s)])
        self.assertEqual(result['observations'][0]['cpu_us_per_payment'], '4503599627370496.5')


    def test_missing_failed_duplicate_samples_remain_failures(self):
        for change in ('missing', 'failed', 'duplicate'):
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                out, m = self.fixture(root)
                if change == 'missing':
                    m['runs'].pop()
                elif change == 'failed':
                    m['runs'][0]['exit'] = 101
                else:
                    m['runs'][-1] = m['runs'][0]
                (out/'manifest.json').write_text(json.dumps(m))
                with self.assertRaises(ValueError):
                    load_evidence(out, root)


if __name__ == '__main__':
    unittest.main()
