#!/usr/bin/env python3
"""Read the saved operator receipts and controller verdicts for an x-series campaign."""
import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import sys

from faults import DEFAULT_DEV, PRECHECK_LEAD, Refusal, commands, load_inventory
from fleet_nodes import paths
from fleet_provision import validate_run_id
from plan import expand
from review_netem import runner_evidence
from runtime import Gate


def require(condition, message):
    if not condition:
        raise ValueError(message)


def timestamp(value):
    return type(value) in (int, float) and math.isfinite(value) and value > 0


def expected_receipts(step, inventory, unit):
    # Reuse the matching runner's pure renderer. Only receipt labels leave this function.
    rendered = commands(step, inventory, unit)
    return [(phase, entry['role'], entry['action'])
            for key, phase in (('apply', 'apply'), ('stopped', 'apply'), ('result', 'apply'),
                               ('restore', 'restore'), ('verify', 'verify'), ('disarm', 'disarm'))
            for entry in rendered.get(key, [])]


def inspect_step(step, controller, rows, expected, start):
    fault_at = int(start) + step['fault_start_offset']
    restore_at = int(start) + step['restore_offset']
    end = int(start) + step['window_end_offset']
    require(controller['action'] == step['action'] and controller['targets'] == step['target_hosts']
            and controller['expect'] == step['expect'], 'Controller step differs from the frozen plan: ' + step['id'])
    checks = [r for r in rows if r['phase'] == 'precheck']
    operations = [r for r in rows if r['phase'] not in ('precheck', 'warn')]
    reasons = []
    if [(r['phase'], r['role'], r['action']) for r in operations] != expected:
        reasons.append('Recorded operations differ from the planned sequence')
    if any(r['ok'] is not True for r in checks + operations):
        reasons.append('An operator check or operation failed')
    if any(r['action'] != 'minting_check' or r['role'] not in step['targets'] or
           r.get('minting_active') is not False or r['planned_at'] != fault_at for r in checks):
        reasons.append('Invalid target precheck')
    if operations:
        recent = {r['role'] for r in checks if fault_at - PRECHECK_LEAD <= r['at'] <= operations[0]['at']}
        if recent != set(step['targets']):
            reasons.append('Missing target precheck near the start of the step')
    if any(b['at'] < a['at'] for a, b in zip(operations, operations[1:])):
        reasons.append('Operator clock moved backwards within the step')
    identities = {(r['phase'], r['role'], r['action'], r['at']) for r in operations}
    if len(identities) != len(operations):
        reasons.append('Duplicate operator receipt')
    for r in operations:
        applying = r['phase'] == 'apply'
        planned, deadline = (fault_at, restore_at) if applying else (restore_at, end)
        if r['planned_at'] != planned or not planned <= r['at'] < deadline:
            reasons.append('Receipt outside its scheduled interval')
            break
    for r in operations:
        if r['action'] == 'stopped' and (r.get('active_state') not in ('inactive', 'failed')
                                         or type(r.get('polls')) is not int or r['polls'] < 1):
            reasons.append('Missing service-stop observation')
            break
    clocks = {}
    if not reasons:
        for role in step['targets']:
            own = [r for r in operations if r['role'] == role]
            applied = max(r['at'] for r in own if r['phase'] == 'apply')
            restored = next(r['at'] for r in own if r['phase'] == 'restore')
            verified = next(r['at'] for r in own if r['phase'] == 'verify')
            clocks[role] = dict(seconds_between_apply_and_restore_receipts=restored - applied,
                                restore_receipt_seconds_after_schedule=restored - restore_at,
                                verify_receipt_seconds_after_schedule=verified - restore_at)
    observation = controller.get('fault_observed')
    observed = observation is True or (step['action'] == 'netem' and observation == 'unknown')
    controller_pass = controller['verdict'] == 'pass' and controller['failures'] == [] and controller['awaiting'] == []
    state = 'complete' if not reasons else 'incomplete'
    if not operations and all(r['ok'] for r in checks):
        state = 'unobserved'
    return dict(step=step['id'], controller_verdict=controller['verdict'], controller_fault_observed=observation,
                operator_receipts=state, reasons=reasons, recorded_operations=len(operations),
                planned_operations=len(expected), operator_clock=clocks,
                recovery_supported=state == 'complete' and observed and controller_pass)


def inspect_fleet(root, report_path=None, dev=DEFAULT_DEV):
    hashes = {}

    def read_bytes(name, path):
        content = path.read_bytes()
        hashes[name] = hashlib.sha256(content).hexdigest()
        return content

    infra = json.loads(read_bytes('infrastructure_plan', root / 'infrastructure-plan.json'))
    run = validate_run_id(infra['run_id'])
    inventory = load_inventory(json.loads(read_bytes('inventory', root / 'inventory.json')))
    plan = json.loads(read_bytes('campaign_plan', root / 'campaign-plan.json'))
    if report_path is None:
        finals = sorted(root.glob('final-report-*.json'))
        require(len(finals) <= 1, 'Several final reports are present. Select an attempt with --report')
        report_path = finals[0] if finals else root / 'result.json'
    report = json.loads(read_bytes('controller_report', report_path))
    activation = json.loads(read_bytes('campaign_start', root / 'campaign-start.json'))
    summary, _ = expand(plan)
    require(report['plan_sha256'] == summary['plan_sha256'] == activation['plan_sha256'],
            'Controller report or activation refers to a different plan')
    attempt = report['run_id']
    require(isinstance(attempt, str) and len(attempt) == len(run) + 2 and attempt[:-2] == run
            and attempt[-2] == '-' and attempt[-1] in 'abcdefghijklmnopqrstuvwxyz',
            'Controller attempt does not belong to this fleet')
    if report_path.name.startswith('final-report-'):
        require(report_path.name == 'final-report-' + attempt + '.json', 'Final report filename names another attempt')
    start = report.get('start')
    require(activation['run_id'] == attempt and timestamp(start) and activation['start'] == start,
            'Controller and activation disagree on the campaign start or attempt')
    require(timestamp(infra.get('created_at')) and timestamp(infra.get('host_end'))
            and infra['created_at'] <= start < infra['host_end'], 'Campaign start falls outside the fleet lifetime')
    steps = summary.get('degrade_steps')
    require(steps, 'No scheduled recovery steps in this plan')
    observed = report['degrade']
    require([r['step'] for r in observed] == [s['id'] for s in steps], 'Missing or unexpected controller steps')

    actions = {s['id']: s['action'] for s in steps}
    journal = defaultdict(list)
    warnings = []
    for line in read_bytes('operator_journal', root / 'faults.jsonl').splitlines():
        row = json.loads(line)
        require(isinstance(row, dict) and timestamp(row.get('at')) and type(row.get('ok')) is bool,
                'Invalid operator receipt')
        if row['phase'] == 'warn' and row['role'] == 'laptop' and (row['step'] == 'preflight' or row['step'] in actions):
            warnings.append({'step': row['step'], 'action': row['action'], 'at': row['at']})
            continue
        require(row['step'] in actions and (row['phase'] != 'warn' or
                (row['action'] == 'qdisc_counters' and actions[row['step']] == 'netem')), 'Unexpected operator receipt')
        journal[row['step']].append(row)
    unit = paths(run)['node_unit']
    results = []
    for step, controller in zip(steps, observed):
        rows = journal[step['id']]
        operations = [r for r in rows if r['action'] != 'qdisc_counters'] if step['action'] == 'netem' else rows
        result = inspect_step(step, controller, operations, expected_receipts(step, inventory, unit), start)
        if step['action'] == 'netem' and result['operator_receipts'] == 'complete':
            try:
                result['network_queue'] = runner_evidence(rows, step, start, dev)
            except (KeyError, TypeError, ValueError, Refusal) as error:
                result['operator_receipts'] = 'incomplete'
                result['recovery_supported'] = False
                result['operator_clock'] = {}
                result['reasons'].append(str(error))
        if controller['verdict'] == 'pass' and not (
                timestamp(report.get('at')) and report['at'] >= start + step['window_end_offset']):
            result['recovery_supported'] = False
            result['reasons'].append('Controller pass predates the end of its recovery window')
        results.append(result)
    return dict(run=run, attempt=attempt, campaign_status=report['status'], input_sha256=hashes, steps=results,
                operator_warnings=warnings, all_recoveries_supported=all(s['recovery_supported'] for s in results))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('fleet', type=Path, help='directory containing the frozen plan, inventory and faults.jsonl')
    parser.add_argument('--report', type=Path, help='controller report (defaults to the final report, then result.json)')
    parser.add_argument('--dev', default=DEFAULT_DEV, help='archived interface name for the D4 command hash check')
    args = parser.parse_args()
    try:
        result = inspect_fleet(args.fleet, args.report, args.dev)
    except (ValueError, KeyError, TypeError, OSError, Refusal, Gate) as error:
        parser.exit(2, f'Invalid recovery evidence: {error}\n')
    print(json.dumps(result, indent=2, sort_keys=True, allow_nan=False))
    return 0 if result['all_recoveries_supported'] else 1


if __name__ == '__main__':
    sys.exit(main())
