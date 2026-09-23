#!/usr/bin/env python3
"""Validate historical resource evidence offline; never grants CT1 acceptance."""
import gzip
import hashlib
import json
from pathlib import Path
import tarfile

from measure_ownership import CASES, records, rss_bytes

HERE = Path(__file__).resolve().parent


def require(condition, message):
    if not condition:
        raise ValueError(message)


def check_cases(directory):
    summary = json.loads((directory / 'summary.json').read_text())
    require(summary['schema'] == 1 and summary['status'] == 'passed', 'incomplete matrix')
    require([c['case'] for c in summary['cases']] == list(CASES), 'case coverage/order')
    for case in summary['cases']:
        name = case['case']
        require(case['exit_code'] == 0 and case['timed_out'] is False
                and case['status'] == 'passed', 'failed case')
        log = (directory / (name + '.log')).read_bytes()
        rss = (directory / (name + '.rss.txt')).read_bytes()
        require(hashlib.sha256(log).hexdigest() == case['output_sha256'], 'raw log hash')
        require(hashlib.sha256(rss).hexdigest() == case['rss_raw_sha256'], 'RSS hash')
        parsed = records(log.decode(), name, CASES[name])
        require(all(parsed[k] == case[k] for k in parsed), 'summary/raw disagreement')
        require(rss_bytes(rss.decode(), summary['environment']['system'])
                == case['whole_case_peak_rss_bytes'], 'RSS unit conversion')
    return summary


def check_linux(directory):
    retention = json.loads((directory / 'retention.json').read_text())
    require(set(retention['files_sha256']) == {p.name for p in directory.iterdir()
                                             if p.name != 'retention.json'}, 'retention inventory')
    for name, digest in retention['files_sha256'].items():
        require(hashlib.sha256((directory / name).read_bytes()).hexdigest() == digest,
                'retained file hash: ' + name)
    summary = check_cases(directory)
    require(summary['environment']['system'] == 'Linux', 'Linux platform')
    require(summary['checkout_commit'] == retention['checkout_merge_sha']
            == summary['ci']['GITHUB_SHA'], 'checkout identity')
    require(summary['ci']['GITHUB_RUN_ID'] == retention['run_id']
            and summary['ci']['GITHUB_RUN_ATTEMPT'] == retention['run_attempt'], 'run identity')
    with tarfile.open(directory / 'sources.tar.gz', 'r:gz') as archive:
        members = archive.getmembers()
        require(len(members) == len(summary['source_sha256'])
                and {m.name for m in members} == set(summary['source_sha256']), 'source inventory')
        for member in members:
            require(member.isfile(), 'source must be regular file')
            require(hashlib.sha256(archive.extractfile(member).read()).hexdigest()
                    == summary['source_sha256'][member.name], 'historical source hash')
    cargo = gzip.decompress((directory / 'cargo.jsonl.gz').read_bytes())
    require(hashlib.sha256(cargo).hexdigest() == summary['cargo_messages_sha256'], 'Cargo hash')
    items = [json.loads(line) for line in cargo.splitlines()]
    artifacts = [x for x in items if x.get('reason') == 'compiler-artifact'
                 and x.get('target', {}).get('name') == 'bth_ct_demurrage_reference'
                 and x.get('executable') and x.get('profile', {}).get('test')]
    require(len(artifacts) == 1 and artifacts[0]['executable'] == summary['executable'],
            'unique measured Cargo test artifact')
    require(any(x.get('reason') == 'build-finished' and x.get('success') is True for x in items),
            'Cargo build incomplete')
    require(all(summary['executable'] in c['command'] for c in summary['cases']), 'case executable')
    return summary


def main():
    mac = check_cases(HERE / 'evidence/ownership-resources-macos')
    linux = check_linux(HERE / 'evidence/ownership-resources-linux')
    fields = ('inputs', 'outputs', 'ring', 'significant_bits', 'multipliers',
              'constraints', 'arithmetic_bytes', 'clsag_field_bytes', 'signatures')
    for a, b in zip(mac['cases'], linux['cases']):
        require([{k: s[k] for k in fields} for s in a['samples']]
                == [{k: s[k] for k in fields} for s in b['samples']], 'cross-platform shape')
    print('PASS: historical macOS/Linux raw evidence; 8 processes and 13 proofs each.')
    print('CT1 integrated acceptance: NOT ESTABLISHED; activation is not authorized.')


if __name__ == '__main__':
    main()
