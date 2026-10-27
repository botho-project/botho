#!/usr/bin/env python3
"""Bounded native testnet campaign; launch config and secrets stay outside the repo."""
import argparse
import asyncio
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
from discovery_plan import degrade_opt_in
from plan import expand
from reporting import legacy_lottery_evidence
from signer_history import (SCAN_OPERATIONS, history_duplicate, merge_scans,
                            prepare_blocks, prepare_spent, scan_payloads)
from runtime import (BlockLog, Gate, HOSTS, Journal, PENDING, Quota, Rpc, atomic, atomic_stream, inventory_chunks,
                     RESTART_ACTIONS, QUORUM_GATE_KEYS, check_identity, check_quorum, loop_delta, check_resources, digest, campaign_targets, campaign_tls_context, pinned_file)


_NO_VIEW = object()

# Owned-output fields the controller never reads (#1633). The signer returns
# each owned output's KEM ciphertext as a JSON list of 1,089 integers, about
# 8.8 KB per output in memory and 5 KB in inventory.json. The same ciphertext
# is kept once, as hex, in the output history (blocks.jsonl) under the same
# output id, which is what every signer request is built from.
INVENTORY_DROPPED_UTXO_FIELDS = frozenset({'kem_ciphertext'})
# inventory.json is evidence only: it is never trusted after a restart, and
# no code reads it back. It is checkpointed on every full sync and otherwise
# at most this often, instead of on every sync (#1633).
INVENTORY_CHECKPOINT_SECONDS = 300


def compact_owned(output):
    """An owned output without the fields the controller never reads.

    Malformed entries are returned unchanged; canonical_inventory() rejects
    them later and keeps them as evidence.
    """
    utxo = output.get('utxo') if isinstance(output, dict) else None
    if isinstance(utxo, dict) and not INVENTORY_DROPPED_UTXO_FIELDS.isdisjoint(utxo):
        output = dict(output, utxo={k: v for k, v in utxo.items()
                                    if k not in INVENTORY_DROPPED_UTXO_FIELDS})
    return output


def parse_cpu_max(text):
    """Whole CPUs granted by a cgroup v2 ``cpu.max`` ("<quota> <period>").

    Rounded down, at least 1. ``None`` when unlimited ("max ...") or
    unparseable, so the caller falls back to the affinity/CPU count.
    """
    fields = text.split()
    if len(fields) != 2 or fields[0] == 'max':
        return None
    try:
        quota, period = int(fields[0]), int(fields[1])
    except ValueError:
        return None
    if quota <= 0 or period <= 0:
        return None
    return max(1, quota // period)


def cgroup_cpu_limit(root='/sys/fs/cgroup', self_cgroup='/proc/self/cgroup'):
    """The tightest ``cpu.max`` on this process's cgroup v2 path, or ``None``.

    systemd's ``CPUQuota=`` lands in the unit's own ``cpu.max``; a parent
    slice may be tighter, so every ancestor below the root is read too.
    """
    try:
        lines = Path(self_cgroup).read_text().splitlines()
    except OSError:
        return None
    path = None
    for line in lines:
        hierarchy, _, rest = line.partition(':')
        controllers, _, relative = rest.partition(':')
        if hierarchy == '0' and controllers == '':
            path = relative.strip()
            break
    if path is None:
        return None
    parts = [p for p in path.split('/') if p]
    limits = []
    for depth in range(len(parts), 0, -1):
        try:
            text = (Path(root).joinpath(*parts[:depth]) / 'cpu.max').read_text()
        except OSError:
            continue
        limit = parse_cpu_max(text)
        if limit is not None:
            limits.append(limit)
    return min(limits) if limits else None


def effective_cpu_count(cgroup_limit=None):
    """CPUs this process may actually use: the cgroup quota, else affinity, else count."""
    limit = cgroup_cpu_limit() if cgroup_limit is None else cgroup_limit
    try:
        available = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        available = os.cpu_count() or 1
    return max(1, min(available, limit) if limit else available)


async def bounded_gather(factories, limit):
    """Await ``factory()`` for each factory, at most ``limit`` at a time.

    Results keep input order. After the first failure no further factory is
    started; calls already running are awaited (never orphaned, so no signer
    subprocess outlives the caller's scan lock), then the failure of the
    lowest-indexed failed call is raised.
    """
    semaphore = asyncio.Semaphore(max(1, limit))
    failed = False
    skipped = object()

    async def one(factory):
        nonlocal failed
        async with semaphore:
            if failed:
                return skipped
            try:
                return await factory()
            except BaseException:
                failed = True
                raise

    results = await asyncio.gather(*(one(f) for f in factories), return_exceptions=True)
    for result in results:
        if isinstance(result, BaseException):
            raise result
    return results
# Offer states that mean a payment inside a step window was missed (#1591).
MISSED = ('skipped', 'unresolved', 'expired', 'rejected', 'aborted')
# The record must start within this long of fault_start, or the controller
# did not observe the step's start (e.g. it was down) and cannot pass it.
DEGRADE_START_SLACK = 60
# Loop metrics are re-read on every tick this close to window_end, so a host
# the next step stops at the same instant still has an end sample.
DEGRADE_END_LEAD = 45
MILESTONES = ('first_reachable_at', 'synced_at_tip_at', 'first_ballot_at', 'first_coinbase_at')


class Controller:
    def __init__(self, state):
        self.state = state
        self.config = json.loads((state/'launch.json').read_text())
        self.plan = json.loads((state/'plan.json').read_text())
        summary, self.events = expand(self.plan)
        # Scheduled validator faults (#1585); absent without the opt-in.
        self.degrade_steps = summary.get('degrade_steps') or []
        self.targets = campaign_targets(self.config, self.plan)
        tls_context = campaign_tls_context(self.config)
        if 'targets' in self.config:
            pinned_file(self.config.get('known_hosts'), self.config.get('known_hosts_sha256'), 'observer known_hosts')
        if 'hosts_file' in self.config or 'hosts_sha256' in self.config:
            pinned_file(self.config.get('hosts_file'), self.config.get('hosts_sha256'), 'hosts file')
        self.j = Journal(state/'journal.sqlite', limits=self.plan['limits'] if self.plan.get('schema_version') == 2 else None)
        if self.j.get('plan_digest',digest(self.plan)) != digest(self.plan):
            raise Gate('manifest changed after initialization')
        self.j.set('plan_digest',digest(self.plan))
        if self.j.get('launch_digest',digest(self.config)) != digest(self.config):
            raise Gate('launch configuration changed after initialization')
        self.j.set('launch_digest',digest(self.config))
        for name,expected in self.config['controller_files'].items():
            if hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()!=expected:
                raise Gate('controller source differs from launch pin')
        if hashlib.sha256(Path(self.config['signer']).read_bytes()).hexdigest() != self.config['signer_sha256']:
            raise Gate('signer artifact changed')
        self.rpc = Rpc(self.j, self.targets, tls_context, allow_env_proxy='targets' not in self.config)
        self.statuses = {}
        self.fresh = 0
        # Append-only history checkpoint (#1633); a legacy blocks.json still loads.
        self.block_log = BlockLog(state)
        self.blocks = self.block_log.load()
        self.wallets = self.config['wallets']
        wallet_count = self.plan['wallets']['count'] if self.plan.get('schema_version') == 2 else 8
        if len(self.wallets) != wallet_count or len({w['address'] for w in self.wallets}) != wallet_count:
            raise Gate('manifest requires distinct dedicated wallets')
        self.inventory = [[] for _ in self.wallets]
        # Never trust persisted wallet inventory after a process restart.
        self.inventory_initialized = False
        self.spent = []
        self.scan_lock = asyncio.Lock()
        self.admission_lock = asyncio.Lock()
        self.reconcile_wakeup = asyncio.Event()
        self.signed_tasks = set()
        self.closed = False

    # -- scheduled degrade-step view (#1587) ---------------------------------
    # One place decides which hosts a plan-scheduled fault may take out. With
    # no active step, live_hosts() is exactly HOSTS and every caller behaves as
    # before; nothing outside an active step's named targets is ever relaxed.

    def degrade_windows(self):
        """Every scheduled step in absolute seconds; empty before campaign start."""
        steps = getattr(self, 'degrade_steps', None)
        start = self.j.get('start') if steps else None
        if start is None:
            return []
        return [{'step': s['id'], 'action': s['action'], 'expect': s['expect'],
                 'offers': s['offers'], 'expected_down': tuple(s['target_hosts']),
                 'fault_start': start+s['fault_start_offset'],
                 'restore_at': start+s['restore_offset'],
                 'window_end': start+s['window_end_offset']} for s in steps]

    def fault_view(self, now=None):
        """The step active at ``now`` (``fault_start <= now < window_end``), or None."""
        now = time.time() if now is None else now
        return next((w for w in self.degrade_windows()
                     if w['fault_start'] <= now < w['window_end']), None)

    def live_hosts(self, now=None, view=_NO_VIEW):
        """HOSTS minus the active step's named targets, in fixed HOSTS order.

        Pass ``view`` (possibly None) to reuse a view already taken this tick.
        """
        view = self.fault_view(now) if view is _NO_VIEW else view
        down = view['expected_down'] if view else ()
        return tuple(h for h in HOSTS if h not in down)

    def resource_window(self, view, host):
        """The check_resources() window (#1586) for a host the active step names."""
        if not view or host not in view['expected_down']:
            return None
        return {k: view[k] for k in ('step','action','fault_start','restore_at','window_end')}

    def fleet_height(self, now=None, default=None):
        """Minimum chain height over live hosts with a current status."""
        live = self.live_hosts(now)
        heights = [s['chainHeight'] for h,s in self.statuses.items()
                   if h in live and 'chainHeight' in s]
        if not heights:
            if default is None:
                raise Gate('fleet status unavailable')
            return default
        return min(heights)

    def safe_halt_seconds(self, since, now):
        """Seconds of [since, now) inside scheduled below-quorum windows."""
        return sum(max(0, min(now, w['window_end'])-max(since, w['fault_start']))
                   for w in self.degrade_windows() if w['expect'] == 'safe_halt')

    def deferred_rows(self):
        """Rows confirmed on the live hosts whose down hosts are not yet verified."""
        return self.j.rows("json_extract(info,'$.deferred_hosts') IS NOT NULL ORDER BY offered, id")

    async def tolerate(self, host, hosts, awaitable, step=None):
        """Await a read; only a host in ``hosts`` (named by a step) may fail."""
        if host not in hosts:
            return await awaitable
        try:
            return await awaitable
        except Quota:
            raise
        except Exception as error:
            self.j.event(None, 'expected_down_unreachable',
                         {'step': step, 'host': host, 'error': str(error)})
            return None

    def rebaseline(self, host, view, baseline, rebased):
        """Persist the one accepted restart of ``host`` in this step.

        The step marker lives in the stored baseline itself, so re-baseline
        and marker are a single journal write: a crash cannot separate them.
        """
        if baseline.get('rebased_step') == view['step']:
            raise Gate(view['step']+': second node restart in one degrade step: '+host)
        with self.j.transaction():
            self.j.set('baseline:'+host, {**rebased, 'rebased_step': view['step']})
            self.j.event(None, 'rebaseline', {'step': view['step'], 'host': host,
                                              'pid': rebased['pid'], 'start': rebased['start']})

    def rejoin_due(self, now):
        """Ended steps whose rejoin has not yet been checked."""
        return [w for w in self.degrade_windows()
                if w['window_end'] <= now and self.j.get('rejoin:'+w['step']) is None]

    def check_rejoin(self, now, statuses, down=(), quorum_gated=False):
        """At window end each named host must be reachable, synced and caught up.

        ``statuses`` maps every host to this tick's ``node_getStatus`` (None if
        unreachable). "Caught up" means at or above the minimum height of the
        hosts neither the ended step nor the active one names. With
        ``quorum_gated`` it must also report the full installed 4-of-5: the
        restart-window allowance for an unevaluated gate ends here (#1608).
        """
        for window in self.rejoin_due(now):
            peers = [s['chainHeight'] for h,s in statuses.items()
                     if s is not None and h not in window['expected_down'] and h not in down]
            for host in window['expected_down']:
                status = statuses.get(host)
                if (status is None or status.get('synced') is not True
                        or (peers and status['chainHeight'] < min(peers))):
                    raise Gate(window['step']+': '+host+' failed to rejoin by window end')
                if quorum_gated:
                    try:
                        check_quorum(status)
                    except Gate as error:
                        raise Gate(window['step']+': '+host+' failed to rejoin by window end: '
                                   +str(error)) from error
            self.j.set('rejoin:'+window['step'], {'at': now, 'heights': {
                h: statuses[h]['chainHeight'] for h in window['expected_down']}})

    # -- degrade-step recovery verdict and measurements (#1591) --------------
    # Journal key degrade:<step>. Times are absolute; reporting converts them
    # to seconds from the planned restore_at (the controller cannot read the
    # runner's faults.jsonl, so the plan is the only restore clock it has).

    async def degrade_evidence(self, now, statuses, prior=None):
        """Record one monitor tick of step evidence; take verdicts at window_end.

        ``statuses`` is this tick's node_getStatus per HOSTS entry (None if
        unreachable); ``prior`` the previous tick's ``self.statuses``. A
        failed criterion stores the failing verdict and raises
        ``Gate('<step>: recovery check failed: <which>')``.
        """
        windows = self.degrade_windows()
        if not windows:
            return
        status = dict(zip(HOSTS, statuses))
        view = self.fault_view(now)
        down = view['expected_down'] if view else ()
        cache = {}

        async def metrics(host, tolerated, step):
            # One read per host per tick. A tolerated failure is None; the
            # same failure re-raises wherever the host is not tolerated.
            if host not in cache:
                try:
                    cache[host] = await self.rpc.metrics(host)
                except Quota:
                    raise
                except Exception as error:
                    cache[host] = error
            value = cache[host]
            if isinstance(value, Exception):
                if not tolerated:
                    raise value
                self.j.event(None, 'expected_down_unreachable',
                             {'step': step, 'host': host, 'error': 'metrics: '+str(value)})
                return None
            return value

        for window in windows:
            if now < window['fault_start']:
                break
            key = 'degrade:'+window['step']
            record = self.j.get(key)
            if record is None and now > window['fault_start']+DEGRADE_START_SLACK:
                # The controller did not see this step start (it was down):
                # its evidence cannot exist, so it is never a pass. Not a
                # halt either; workload_complete() and coverage stay false.
                record = self.degrade_unobserved(window, now)
            if record is None:
                record = await self.degrade_open(window, now, status, prior or {}, metrics)
            if record.get('verdict') is not None:
                continue
            if record.get('closed_at') is None:
                await self.degrade_track(record, window, now, status, metrics)
                if now >= window['window_end']:
                    await self.degrade_close(record, window, now, status, down, metrics)
                else:
                    self.j.set(key, record)
            if record.get('closed_at') is not None and record.get('verdict') is None:
                self.degrade_settle(record, window, now)

    def live_tip(self, statuses, named):
        """Live-fleet height: min chainHeight over reachable hosts a step does not name."""
        heights = [s['chainHeight'] for h,s in statuses.items()
                   if s is not None and h not in named and 'chainHeight' in s]
        return min(heights) if heights else None

    def degrade_unobserved(self, window, now):
        record = {'step': window['step'], 'action': window['action'],
                  'expect': window['expect'], 'targets': list(window['expected_down']),
                  'fault_start': window['fault_start'], 'restore_at': window['restore_at'],
                  'window_end': window['window_end'], 'opened_at': now, 'hosts': {},
                  'closed_at': now, 'verdict': {'status': 'unobserved', 'at': now,
                                                'failures': ['step start not observed']}}
        self.j.set('degrade:'+window['step'], record)
        self.j.event(None, 'degrade_verdict', {'step': window['step'], **record['verdict']})
        return record

    async def degrade_open(self, window, now, status, prior, metrics):
        named = window['expected_down']
        # Live hosts must answer; a failure retries the whole record next tick.
        start = {h: await metrics(h, h in named, window['step']) for h in HOSTS}
        record = {
            'step': window['step'], 'action': window['action'],
            'expect': window['expect'], 'targets': list(named),
            'fault_start': window['fault_start'], 'restore_at': window['restore_at'],
            'window_end': window['window_end'], 'opened_at': now,
            'metrics_start': start, 'metrics_end': {},
            'tip_fault_start': self.live_tip(status, named),
            'tip_restore': None, 'tip_resumed_at': None, 'tips': {}, 'conflict': None,
            'hosts': {h: {'last_slot': (prior.get(h) or {}).get('lastExternalizedSlot'),
                          'peer_zero': False, **dict.fromkeys(MILESTONES),
                          'reachable_height': None, 'synced_height': None}
                      for h in named},
            'closed_at': None, 'verdict': None,
        }
        self.j.set('degrade:'+window['step'], record)
        return record

    async def degrade_track(self, record, window, now, status, metrics):
        named = window['expected_down']
        tip = self.live_tip(status, named)
        if window['expect'] == 'safe_halt':
            for s in status.values():
                if s is None or type(s.get('chainHeight')) is not int or not s.get('tipHash'):
                    continue
                seen = record['tips'].setdefault(str(s['chainHeight']), s['tipHash'])
                if seen != s['tipHash'] and record['conflict'] is None:
                    record['conflict'] = {'height': s['chainHeight'], 'hashes': [seen, s['tipHash']]}
        if now < window['restore_at']:
            # Start samples a named host missed while already down.
            for host in named:
                if record['metrics_start'].get(host) is None:
                    record['metrics_start'][host] = await metrics(host, True, window['step'])
        elif record['tip_restore'] is None:
            record['tip_restore'] = tip
        elif (record['tip_resumed_at'] is None and tip is not None
                and tip > record['tip_restore']):
            record['tip_resumed_at'] = now
        for host in named:
            s, track = status.get(host), record['hosts'][host]
            if now < window['restore_at']:
                if s is None:
                    continue
                if s.get('lastExternalizedSlot') is not None:
                    track['last_slot'] = s['lastExternalizedSlot']
                if s.get('scpPeerCount') == 0 or s.get('peerCount') == 0:
                    track['peer_zero'] = True
                continue
            if s is None:
                continue
            height = s.get('chainHeight')
            if track['first_reachable_at'] is None:
                track.update(first_reachable_at=now, reachable_height=height)
            if (track['synced_at_tip_at'] is None and s.get('synced') is True
                    and type(height) is int and tip is not None and height >= tip):
                track.update(synced_at_tip_at=now, synced_height=height)
            slot = s.get('lastExternalizedSlot')
            index = s.get('scpSlotIndex')
            # With no pre-fault slot seen, the live tip at restore is the
            # baseline: a node's stale pre-fault slot is not a new ballot.
            baseline = track['last_slot'] if track['last_slot'] is not None else record['tip_restore']
            if track['first_ballot_at'] is None and (
                    (type(slot) is int and baseline is not None and slot > baseline)
                    or ((s.get('scpBallotCounter') or 0) > 0 and type(index) is int
                        and record['tip_restore'] is not None and index > record['tip_restore'])):
                track['first_ballot_at'] = now
            # Only the producer mints and decision 4 never names it (x1).
            track['first_coinbase_at'] = None
        if now >= window['window_end']-DEGRADE_END_LEAD:
            for host in HOSTS:
                sample = await metrics(host, True, window['step'])
                if sample is not None:
                    record['metrics_end'][host] = sample

    def fault_observed(self, record, window):
        """True/False from controller observations; 'unknown' for netem."""
        if window['action'] in RESTART_ACTIONS:
            return all((self.j.get('baseline:'+h) or {}).get('rebased_step') == window['step']
                       for h in window['expected_down'])
        if window['action'] == 'isolate':
            return all(record['hosts'][h]['peer_zero'] for h in window['expected_down'])
        return 'unknown'  # netem: operator reconciles faults.jsonl post-run

    async def degrade_close(self, record, window, now, status, down, metrics):
        """The window_end verdict: every criterion observable at this tick."""
        step, named = window['step'], window['expected_down']
        failures = []
        observed = self.fault_observed(record, window)
        record['fault_observed'] = observed
        if observed is False:
            failures.append('fault not exercised')
        # One tip: every reachable host agrees at the common minimum height.
        # Only a host a later, active step names may be absent here.
        reachable = [h for h in HOSTS if status.get(h) is not None]
        missing = [h for h in HOSTS if h not in reachable and h not in down]
        if missing:
            failures.append('one tip: no status from '+', '.join(missing))
        elif reachable:
            height = min(status[h]['chainHeight'] for h in reachable)
            blocks = await asyncio.gather(*(self.tolerate(h, down, self.rpc.call(
                h, 'getBlockByHeight', {'height': height}), step) for h in reachable))
            hashes = {b.get('hash') for b in blocks if b is not None}
            record['tip_check'] = {'height': height, 'hosts': [h for h,b in zip(reachable,blocks) if b is not None]}
            if len(hashes) != 1:
                failures.append('one tip: conflicting hashes at height '+str(height))
        # Loop stalls: over-1 s recorded only; any >= 5 s delta fails.
        loops = {}
        for host in HOSTS:
            sample = await metrics(host, host in down, step)
            if sample is not None:
                record['metrics_end'][host] = sample
            end = record['metrics_end'].get(host)
            restarted = (host in named and window['action'] in RESTART_ACTIONS
                         and (self.j.get('baseline:'+host) or {}).get('rebased_step') == step)
            start = record['metrics_start'].get(host)
            if end is None or (start is None and not restarted):
                loops[host] = None
                failures.append('loop metrics unavailable: '+host)
                continue
            loops[host] = loop_delta(start, end, restarted)
            if loops[host]['delayed'] > 0:
                failures.append('node_loop_delayed on '+host)
        record['loops'] = loops
        if window['expect'] == 'safe_halt':
            if record['conflict'] is not None:
                failures.append('conflicting hash at height '+str(record['conflict']['height']))
            if record['tip_resumed_at'] is None:
                failures.append('tip did not advance after restore')
        record['closed_at'] = now
        # Missed payments, deferred receipts and accounting are checked by
        # degrade_settle(), which runs straight after this in the same tick.
        if failures:
            self.degrade_verdict(record, failures, now)
        # Passed at window_end; degrade_settle closes the after-window part.
        self.j.set('degrade:'+step, record)

    def missed_offers(self, window):
        return self.j.rows("kind='campaign' AND offered>=? AND offered<? AND state IN ("
                           + ','.join('?' for _ in MISSED)+") ORDER BY offered, id",
                           (window['fault_start'], window['window_end'], *MISSED))

    def degrade_settle(self, record, window, now):
        """Close an open verdict once its after-window evidence exists.

        Payments, deferred receipts and exact accounting resolve after
        window_end: the in-window offers must all reconcile, the step's
        deferred receipts verify (#1587), and the first accounting snapshot
        at or after window_end show difference 0. Until then it stays open.
        """
        missed = self.missed_offers(window)
        if missed:
            return self.degrade_verdict(record, ['missed payment '+missed[0]['id']], now)
        unfinished = self.j.rows("kind='campaign' AND offered>=? AND offered<? AND state!='reconciled'",
                                 (window['fault_start'], window['window_end']))
        deferred = [r for r in self.deferred_rows()
                    if json.loads(r['info']).get('deferred_step') == window['step']]
        snapshot = self.j.get('accounting')
        # After a controller restart the blocks reload from disk but the
        # wallet inventory is empty and untrusted until the first sync(),
        # and the monitor (this caller) starts before that sync(). Accounting
        # against that inventory would be a false mismatch, so wait for it.
        if (snapshot is None or snapshot.get('at', 0) < window['window_end']) \
                and not self.j.pending() and self.blocks and self.inventory_initialized:
            try:
                self.accounting()
            except Gate as error:
                return self.degrade_verdict(record, ['accounting: '+str(error)], now)
            snapshot = self.j.get('accounting')
        if snapshot is not None and snapshot.get('at', 0) >= window['window_end'] \
                and snapshot.get('difference') != 0:
            return self.degrade_verdict(
                record, ['accounting difference '+str(snapshot.get('difference'))], now)
        record['awaiting'] = [*(['payments'] if unfinished else []),
                              *(['deferred receipts'] if deferred else []),
                              *(['accounting'] if snapshot is None
                                or snapshot.get('at', 0) < window['window_end'] else [])]
        if record['awaiting']:
            self.j.set('degrade:'+window['step'], record)
            return None
        return self.degrade_verdict(record, [], now)

    def degrade_verdict(self, record, failures, now):
        step = record['step']
        if failures:
            record['verdict'] = {'status': 'fail', 'at': now, 'failures': failures}
            self.j.set('degrade:'+step, record)
            self.j.event(None, 'degrade_verdict', {'step': step, **record['verdict']})
            raise Gate(step+': recovery check failed: '+'; '.join(failures))
        record['verdict'] = {'status': 'pass', 'at': now, 'failures': []}
        self.j.set('degrade:'+step, record)
        self.j.event(None, 'degrade_verdict', {'step': step, **record['verdict']})
        return record['verdict']

    def settle_degrade_verdicts(self, now=None):
        """Settle open verdicts outside the monitor (final drain)."""
        now = time.time() if now is None else now
        for window in self.degrade_windows():
            record = self.j.get('degrade:'+window['step'])
            if record and record.get('closed_at') is not None and record.get('verdict') is None:
                self.degrade_settle(record, window, now)

    def gate(self):
        if self.j.get('status') not in ('setup','running'):
            raise Gate('run is held: '+str(self.j.get('reason')))
        if (self.state/'STOP').exists():
            raise Gate('operator stop marker')
        if time.time()-self.fresh > 60:
            raise Gate('fleet or observer data stale')
        view = self.fault_view()
        if view and view['expect'] == 'safe_halt':
            raise Quota(view['step']+': scheduled below-quorum window; admission paused')
        down = view['expected_down'] if view else ()
        if any(not s.get('synced') for h,s in self.statuses.items() if h not in down):
            raise Quota('fleet is recovering sync')
        deadline = self.j.get('end',self.j.get('setup_start')+self.plan.get('setup_deadline_hours',8)*3600)
        if time.time() >= deadline:
            raise Gate('immutable admission deadline')

    def halt(self, reason):
        if self.j.get('status') in ('setup','running'):
            self.j.set('status','held')
            self.j.set('reason',str(reason))
            self.j.set('stopped_at',time.time())
            self.j.event(None,'held',str(reason))
            self.report()

    def scan_parallelism(self, incremental):
        """Concurrent native wallet scans (#1629).

        Whole-chain scans (``restore_check``, or a first ``scan`` from
        genesis) run one at a time: each holds the full history in its
        request and a ~300 MB signer, and the controller unit has no swap,
        a bounded ``MemoryMax`` (1G on x1, 3G since #1633) and
        ``CPUQuota=100%``. Only incremental
        scans of appended blocks run in parallel, up to ``scan_workers`` or
        the effective CPU limit (cgroup ``cpu.max``), which is 1 on x1.
        """
        if not incremental:
            return 1
        explicit = getattr(self, 'scan_workers', None)
        return max(1, explicit or effective_cpu_count())

    async def native(self, request):
        # Whole-history scans are split into bounded consecutive chunks; the
        # pinned signer's MAX_REQUEST is 128 MiB (#1631). Ownership is
        # per-output, so concatenated chunk results equal one request's.
        if request.get('operation') in SCAN_OPERATIONS:
            duplicate = history_duplicate(request['blocks'])
            if duplicate:
                raise Gate('signer: '+duplicate)
            results = [await self.native_payload(payload) for payload in scan_payloads(request)]
            try:
                return merge_scans(results)
            except ValueError as error:
                raise Gate(str(error))
        return await self.native_payload(json.dumps(request).encode())

    def prepare_history(self, height, selected):
        """Bounded ``blocks`` and ``spent`` for a prepare request (#1631)."""
        return (prepare_blocks(self.blocks, height, [o['id'] for o in selected]),
                prepare_spent(self.spent, {o.get('key_image') for o in selected} - {None}))

    async def native_payload(self, payload):
        def run():
            result = subprocess.run([self.config['signer']], input=payload,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=90, check=False)
            if len(result.stdout) > 32*1024*1024:
                raise Gate('signer response bound')
            value = json.loads(result.stdout)
            if result.returncode or not value.get('ok'):
                raise Gate('signer: '+value.get('error','failed'))
            return value['result']
        return await asyncio.to_thread(run)

    async def observer(self, host):
        proc = await asyncio.create_subprocess_exec('ssh','-F','/dev/null','-i',self.config['observer_key'],
            '-o','BatchMode=yes','-o','IdentitiesOnly=yes','-o','StrictHostKeyChecking=yes','-o','ConnectTimeout=5',
            '-o','UserKnownHostsFile='+self.config['known_hosts'],self.targets[host]['observer_ssh_target'],
            stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.PIPE)
        try:
            out, _ = await asyncio.wait_for(proc.communicate(),10)
        except BaseException:
            proc.kill()
            await proc.wait()
            raise
        if proc.returncode or len(out)>128*1024:
            raise Gate('restricted observer export failed: '+host)
        return json.loads(out)

    async def monitor(self):
        while not self.closed:
            started = time.monotonic()
            try:
                # One view per tick: a host the active step names may be
                # unreachable; every other host still fails closed.
                now = time.time()
                view = self.fault_view(now)
                step = view['step'] if view else None
                down = view['expected_down'] if view else ()
                # A host whose window just ended may still be unreachable;
                # its status read reports that as the rejoin failure below.
                rejoining = {h for w in self.rejoin_due(now) for h in w['expected_down']}
                # Plan-level, not per host: only x-series (degrade opt-in)
                # plans require the installed 4-of-5 quorum (#1605).
                quorum_gated = degrade_opt_in(self.plan)
                statuses = await asyncio.gather(*(self.tolerate(h,{*down,*rejoining},self.rpc.call(h,'node_getStatus'),step) for h in HOSTS))
                records = await asyncio.gather(*(self.tolerate(h,down,self.observer(h),step) for h in HOSTS))
                self.check_rejoin(now,dict(zip(HOSTS,statuses)),down,quorum_gated)
                for host,status,observed in zip(HOSTS,statuses,records):
                    if status is not None:
                        check_identity(status,self.plan)
                        # Degrade-phase (x-series) plans need the installed
                        # 4-of-5, f=1 quorum on every reachable validator; a
                        # silent 4-of-4 would void D1-D5 (#1605).
                        if quorum_gated:
                            # A host the active restart step names may serve
                            # RPC before its gate's first evaluation (#1608);
                            # isolate/netem never restart, rejoining is strict.
                            unevaluated = (view is not None and host in down
                                           and view['action'] in RESTART_ACTIONS)
                            check_quorum(status,allow_unevaluated=unevaluated)
                            if unevaluated and all(status.get(k,0) is None for k in QUORUM_GATE_KEYS):
                                self.j.event(None,'quorum_unevaluated',{'step':step,'host':host})
                    baseline = self.j.get('baseline:'+host)
                    if baseline is None:
                        if not observed or observed[-1].get('error'):
                            raise Gate('observer baseline unavailable')
                        baseline = self.config.get('resource_baselines',{}).get(host,observed[-1])
                        if baseline['binary'] != self.config['node_sha256']:
                            raise Gate('deployed binary differs from launch pin')
                        self.j.set('baseline:'+host,baseline)
                    if observed is not None:
                        rebased = check_resources(observed,baseline,time.time(),self.resource_window(view,host))
                        if rebased is not None:
                            self.rebaseline(host,view,baseline,rebased)
                    # No progress is expected from a named host until window
                    # end, nor from anyone while quorum is lost (safe_halt).
                    if host in down or (view and view['expect'] == 'safe_halt'):
                        self.j.set('unsynced:'+host,None)
                    elif not status.get('synced'):
                        since = self.j.get('unsynced:'+host) or time.time()
                        self.j.set('unsynced:'+host,since)
                        if time.time()-since > 120:
                            raise Gate('node unsynced for two minutes: '+host)
                    else:
                        self.j.set('unsynced:'+host,None)
                    # Post-idle memory is evaluated once per completed active cycle.
                    # A stopped node's error row carries no memory sample.
                    if observed and not observed[-1].get('error'):
                        self.j.set('latest:'+host,observed[-1])
                # Hash agreement covers every reachable host, including a named
                # host that is isolated or delayed and therefore behind.
                reachable = [(h,s) for h,s in zip(HOSTS,statuses) if s is not None]
                minimum = min(s['chainHeight'] for _,s in reachable)
                if self.j.get('checkpoint_height') != minimum:
                    checkpoints = await asyncio.gather(*(self.tolerate(h,down,self.rpc.call(h,'getBlockByHeight',{'height':minimum}),step) for h,_ in reachable))
                    checkpoints = [b for b in checkpoints if b is not None]
                    if len({b['hash'] for b in checkpoints}) != 1:
                        raise Gate('conflicting block hashes at the same finalized height')
                    self.j.set('checkpoint_height',minimum)
                    self.j.set('checkpoint_hash',checkpoints[0]['hash'])
                if not self.j.get('genesis_verified'):
                    genesis = await asyncio.gather(*(self.tolerate(h,down,self.rpc.call(h,'getBlockByHeight',{'height':0}),step) for h,_ in reachable))
                    if any(b.get('hash') != self.plan['genesis'] for b in genesis if b is not None):
                        raise Gate('genesis differs from manifest')
                    # Verified only once every host has answered.
                    if None not in genesis and len(genesis) == len(HOSTS):
                        self.j.set('genesis_verified',True)
                # Per-step recovery evidence and window_end verdicts (#1591).
                await self.degrade_evidence(now,statuses,self.statuses)
                previous_height = self.fleet_height(default=-1)
                self.statuses = {h: {'expected_down': True} if s is None
                                 else {**s,'expected_down': True} if h in down else s
                                 for h,s in zip(HOSTS,statuses)}
                self.fresh = time.time()
                self.j.set('fleet_latest',{'at':self.fresh,'nodes':self.statuses})
                live_height = self.fleet_height()
                self.j.event(None,'fleet',{'height':minimum,'at':self.fresh,
                    **({'step':view['step'],'expected_down':list(down),'live_height':live_height} if view else {})})
                self.memory_cycle()
                # Controller shares no node resources; still stop if its own
                # state volume approaches exhaustion or its evidence grows too far.
                if shutil.disk_usage(self.state).free < 2*1024**3:
                    raise Gate('controller disk headroom')
                if live_height > previous_height:
                    self.reconcile_wakeup.set()
            except Quota as error:
                self.j.event(None,'monitor_quota',str(error))
            except Gate as error:
                self.halt(error)
            except Exception as error:
                self.j.event(None,'monitor_error',str(error))
                if time.time()-self.fresh > 60:
                    self.halt('monitor unavailable for sixty seconds')
            await asyncio.sleep(max(.2,15-(time.monotonic()-started)))

    def memory_cycle(self):
        last = self.j.get('last_write',0)
        now = time.time()
        pending = bool(self.j.pending())
        view = self.fault_view(now)
        down = view['expected_down'] if view else ()
        for host in HOSTS:
            if self.j.get('rss_cycles:'+host,0) >= 3:
                self.halt('post-idle RSS growth across three cycles: '+host)
            row,base = self.j.get('latest:'+host), self.j.get('baseline:'+host)
            status = self.statuses.get(host,{})
            state = self.j.get('rss_idle:'+host,{})
            # Only continuously observed idle time qualifies. New writes and
            # observation gaps cannot inherit an earlier five-minute window.
            if (state.get('write') != last or state.get('sample_at') is None
                    or not 0 <= now-state['sample_at'] <= 60):
                state['since'] = None
            state.update(write=last,sample_at=now)
            # A host the active step names is never idle evidence: its
            # samples straddle a stop, isolation or netem, not a quiet cycle.
            idle = (host not in down and not pending and 0 <= now-self.fresh <= 60 and row and base
                    and 0 <= now-row['at'] <= 60 and status.get('synced') is True
                    and status.get('mintingActive') is False
                    and type(status.get('mempoolSize')) is int and status['mempoolSize']==0)
            if not idle:
                state['since'] = None
            elif state.get('since') is None:
                state['since'] = now
            self.j.set('rss_idle:'+host,state)
            # Each host completes a write cycle independently: an active
            # producer must not consume a passive peer's idle observation, or
            # be compared against its own fixed idle RSS baseline while mining.
            completed = self.j.get('memory_cycle:'+host,self.j.get('memory_cycle',0))
            if (not last or state['since'] is None or now-state['since'] < 300
                    or completed >= last):
                continue
            high = row['rss']-base['rss'] > 64*1024**2 and row['rss'] > base['rss']*1.2
            count = self.j.get('rss_cycles:'+host,0)+1 if high else 0
            with self.j.transaction():
                self.j.set('rss_cycles:'+host,count)
                self.j.set('memory_cycle:'+host,last)
            if count >= 3:
                self.halt('post-idle RSS growth across three cycles: '+host)

    async def sync(self, full=False, *, draining=False, rescan_wallets=None):
        """Refresh output history, wallet ownership and key-image state.

        ``full`` refetches the whole output history from genesis, requires it
        to extend the checkpointed history exactly, and rechecks every key
        image. ``rescan_wallets`` (default: ``full``) rebuilds every wallet's
        ownership with a native ``restore_check`` over all blocks, the
        independent derivation proof whose cost grows with the chain.

        ``full=True, rescan_wallets=False`` is the phase-boundary mode
        (#1629): the history refetch and checkpoint comparison, the
        duplicate-output-id rule and the full key-image recheck are kept,
        but ownership is extended by an incremental native scan of only the
        blocks beyond the checkpointed history. It falls back to the full
        ``restore_check`` whenever no inventory was scanned in this process
        (persisted inventory is never trusted after a restart).
        """
        rescan = full if rescan_wallets is None else rescan_wallets
        if rescan and not full:
            raise ValueError('a wallet restore rescan requires a full history refetch')
        async with self.scan_lock:
            if not self.statuses:
                raise Gate('fleet status unavailable')
            # Live hosts only: a named host that is down or behind must not
            # freeze the scan height (#1587). Equals min over HOSTS otherwise.
            height = self.fleet_height()
            start = 0 if full else (self.blocks[-1]['height']+1 if self.blocks else 0)
            new = []
            waited = 0.0

            async def read(host, method, params, timeout=10):
                nonlocal waited
                # Only these idempotent reads wait for the shared durable quota.
                # Reserve ten requests/minute for monitoring (or all but one
                # slot when the advertised endpoint quota is smaller).
                while True:
                    # STOP closes admission, not read-only receipt/final scans.
                    # Shutdown and the quota wait bound still apply to drains.
                    if self.closed or (not draining and (self.state / "STOP").exists()):
                        raise Gate("scan stopped during quota wait")
                    try:
                        now = self.j.clock()
                        count = self.j.db.execute(
                            "SELECT COUNT(*) FROM requests WHERE host=? AND at>?",
                            (host, now - 60),
                        ).fetchone()[0]
                        limit = min(self.j.limits["max_rpc_per_endpoint_per_minute"], self.j.get("quota:" + host, 100) // 2)
                        if count >= max(1, limit - 10):
                            raise Quota("scan reserves monitoring headroom")
                        return await self.rpc.call(
                            host, method, params, timeout=timeout
                        )
                    except Quota:
                        if waited >= 300:
                            raise Gate("scan quota wait exceeded five minutes")
                        before = time.monotonic()
                        await asyncio.sleep(min(1, 300 - waited))
                        waited += time.monotonic() - before

            previous = self.blocks
            # A full refetch is compared with the checkpoint chunk by chunk and
            # only the blocks beyond it are kept: the whole refetched history
            # is never held next to the checkpoint (#1633). The outcome equals
            # ``new[:len(previous)] != previous`` over the whole refetch.
            diverged = False
            for first in range(start,height+1,50):
                last = min(height,first+49)
                # Rotation is deterministic over the live list, re-read per
                # chunk so a step starting mid-scan never routes to its host.
                live = self.live_hosts()
                blocks = await read(
                    live[(first // 50) % len(live)],
                    "chain_getOutputs",
                    {"start_height": first, "end_height": last},
                    timeout=20,
                )
                if [b['height'] for b in blocks] != list(range(first,last+1)):
                    raise Gate('incomplete or unordered output range')
                if full:
                    overlap = max(0, min(len(blocks), len(previous)-first))
                    if blocks[:overlap] != previous[first:first+overlap]:
                        diverged = True
                    blocks = blocks[overlap:]
                new.extend(blocks)
            if full:
                if previous and (diverged or height+1 < len(previous)):
                    raise Gate('full rescan disagrees with checkpointed output history')
            # The (refetched) history beyond the verified checkpoint. With no
            # new blocks keep the checkpointed list itself, not an equal copy:
            # the block log appends only to the exact list it last persisted,
            # so a fresh copy would turn the next append into a whole-file
            # rewrite (#1633 review).
            blocks = previous+new if new else previous
            appended = new
            incremental = self.inventory_initialized and not rescan
            restore = full and not incremental
            scan_blocks = appended if incremental else blocks
            added = set()
            if incremental:
                # The native scanner rejects duplicate output IDs within its
                # request. Preserve that rule across separate append requests.
                # The ids of the checkpointed history are cached across syncs
                # and extended only on success (#1633).
                seen = self.history_output_ids(previous)
                for block in appended:
                    for output in block['outputs']:
                        identifier = (output['txHash'],output['outputIndex'])
                        if identifier in seen or identifier in added:
                            raise Gate('duplicate output id across incremental history')
                        added.add(identifier)

            async def scan_wallet(index, wallet):
                owned = {o['id']:o for o in self.inventory[index]} if incremental else {}
                if scan_blocks or not incremental:
                    scanned = await self.native({'operation':'restore_check' if restore else 'scan',
                        'wallet':wallet['key'], 'blocks':scan_blocks,
                        **({'expected_address':wallet['address']} if restore else {})})
                    if scanned['address'] != wallet['address']:
                        raise Gate('wallet identity differs from allowlist')
                    for output in scanned['owned']:
                        if output['id'] in owned:
                            raise Gate('duplicate owned output across incremental scans')
                        owned[output['id']] = compact_owned(output)
                return list(owned.values())

            # Separate signer processes and derivations: restores never reuse
            # key state. Each wallet's request, subprocess and result are
            # independent; incremental scans may run with bounded parallelism,
            # whole-chain scans stay serial (#1629). Results keep wallet order
            # and any failure fails the whole scan.
            scans = await bounded_gather(
                [lambda i=i, w=w: scan_wallet(i, w) for i,w in enumerate(self.wallets)],
                self.scan_parallelism(incremental))
            all_images = list(dict.fromkeys(o['key_image'] for owned in scans for o in owned))
            # Finalized spent images stay spent. Phase/full scans independently
            # recheck them; routine scans reserve RPC capacity for live inputs.
            cached = {} if full else {s['keyImage']:s for s in self.spent if s['spent']}
            query_images = [image for image in all_images if image not in cached]
            checked = dict(cached)
            for first in range(0,len(query_images),256):
                images = query_images[first:first+256]
                live = self.live_hosts()
                host=live[(first//256+height)%len(live)]
                found = await read(
                    host, "chain_areKeyImagesSpent", {"keyImages": images}
                )
                if ([s.get('keyImage') for s in found] != images or any('error' in s or
                    type(s.get('spent')) is not bool or type(s.get('pending')) is not bool for s in found)):
                    raise Gate('malformed spent-image response')
                checked.update((s['keyImage'],s) for s in found)
            spent=[checked[image] for image in all_images]
            # Do not advance in-memory scan progress on a failed native/RPC
            # check: retry must scan the same new outputs, never skip them.
            if full or new:
                self.history_log().persist(previous, blocks)
            self.checkpoint_inventory(height, scans, spent, force=full)
            self.blocks, self.inventory, self.spent = blocks, scans, spent
            self.inventory_initialized = True
            if incremental:
                seen.update(added)
                self.output_id_cache = (blocks, len(blocks), seen)
            return height

    def history_log(self):
        log = self.__dict__.get('block_log')
        if log is None:
            log = self.block_log = BlockLog(self.state)
        return log

    def history_output_ids(self, blocks):
        """Every (txHash, outputIndex) in ``blocks``; cached per history list (#1633)."""
        cached = self.__dict__.get('output_id_cache')
        if cached and cached[0] is blocks and cached[1] == len(blocks):
            return cached[2]
        ids = {(o['txHash'],o['outputIndex']) for b in blocks for o in b['outputs']}
        self.output_id_cache = (blocks, len(blocks), ids)
        return ids

    def checkpoint_inventory(self, height, owned, spent, force=False):
        """Atomically write inventory.json, at most every INVENTORY_CHECKPOINT_SECONDS.

        Evidence only (never trusted after a restart). Always written on a
        full sync and on the first sync of a process.
        """
        now = time.monotonic()
        last = self.__dict__.get('inventory_checkpoint_at')
        if not force and last is not None and now-last < INVENTORY_CHECKPOINT_SECONDS:
            return
        atomic_stream(self.state/'inventory.json', inventory_chunks(time.time(), height, owned, spent))
        self.inventory_checkpoint_at = now

    def canonical_history(self):
        # Match target-only ledger resolution before any maturity/age filtering.
        # History and inventory retain unsupported receipts for accounting.
        canonical = {}
        for block in sorted(self.blocks, key=lambda block: block['height']):
            for output in block['outputs']:
                key = output['targetKey'].lower()
                if not output.get('lottery', False) and key not in canonical:
                    canonical[key] = (block['height'], output)
        return canonical

    def canonical_inventory(self, wallet, canonical=None):
        canonical = self.canonical_history() if canonical is None else canonical
        seen = set()
        outputs = []
        for output in self.inventory[wallet]:
            # The signer inventory is also retained verbatim for accounting.
            # Reject malformed entries here without rewriting that evidence.
            if not isinstance(output, dict) or not isinstance(output.get('utxo'), dict):
                continue
            utxo = output['utxo']
            if (
                not isinstance(output.get('id'), str)
                or not isinstance(output.get('key_image'), str)
                or not output['key_image']
                or utxo.get('lottery', False) is not False
                or any(
                    type(utxo.get(field)) is not int or utxo[field] < 0
                    for field in ('created_at', 'output_index', 'amount')
                )
            ):
                continue
            if any(
                not isinstance(utxo.get(field), list)
                or len(utxo[field]) != 32
                or any(
                    type(byte) is not int or not 0 <= byte <= 255
                    for byte in utxo[field]
                )
                for field in ('target_key', 'tx_hash')
            ):
                continue
            key = bytes(utxo['target_key']).hex()
            tx_hash = bytes(utxo['tx_hash']).hex()
            if key == '0' * 64 or output['id'] != tx_hash + ':' + str(
                utxo['output_index']
            ):
                continue
            original = canonical.get(key)
            if utxo.get('lottery', False) or original is None or key in seen:
                continue
            created, row = original
            if (
                created != utxo['created_at']
                or row.get('txHash', '').lower() != tx_hash
                or row.get('outputIndex') != utxo['output_index']
            ):
                continue
            seen.add(key)
            outputs.append(output)
        return outputs

    def unspent_setup_inputs(self, wallet, canonical=None):
        """Existing distinct inputs, including those awaiting maturity or decoys.

        This count only prevents redundant splits; admission still requires
        spendable's eligibility checks and every wallet's four-input signer probe.
        """
        state = {s['keyImage']: s for s in self.spent}
        reserved = {r[0] for r in self.j.db.execute('SELECT input FROM reservations')}
        outputs, seen_ids, seen_images = [], set(), set()
        for output in self.canonical_inventory(wallet, canonical):
            status = state.get(output['key_image'])
            if (
                not status
                or status.get('spent') is not False
                or status.get('pending') is not False
                or 'error' in status
                or output['id'] in reserved
                or output['id'] in seen_ids
                or output['key_image'] in seen_images
            ):
                continue
            outputs.append(output)
            seen_ids.add(output['id'])
            seen_images.add(output['key_image'])
        return outputs

    def spendable(self, wallet, height, amount=0, count=1):
        canonical = self.canonical_history()
        outputs = []
        decoys = {}
        for output in self.unspent_setup_inputs(wallet, canonical):
            utxo = output['utxo']
            if height-utxo['created_at'] < 10:
                continue
            age = height-utxo['created_at']
            # Match production age_similarity_band: integer +/-10%, ten-block floor.
            delta = age//10
            low,high = max(10,age-delta),age+delta
            keys = {key for key, (created, _) in canonical.items()
                    if low <= height-created <= high and key != '0'*64}
            keys.discard(bytes(utxo['target_key']).hex())
            if len(keys) >= 19:
                outputs.append(output)
                decoys[output['id']] = keys
        outputs.sort(key=lambda u:u['utxo']['amount'],reverse=True)
        # Lottery history can reuse a target key. Distinct RPC IDs do not make
        # two independently spendable inputs when their key images coincide.
        unique=[]
        seen=set()
        for output in outputs:
            if output['key_image'] not in seen:
                unique.append(output)
                seen.add(output['key_image'])
        outputs=unique
        if amount:
            selected = outputs[:count]
            if len(selected)<count or sum(o['utxo']['amount'] for o in selected) < amount+5_000_000_000:
                return []
            # Match the native signer: no selected real target may be a decoy
            # in any ring. Keep the deterministic largest-value prefix; wait
            # if it is infeasible rather than searching alternative input sets.
            excluded = {bytes(o['utxo']['target_key']).hex() for o in selected}
            if any(len(decoys[o['id']] - excluded) < 19 for o in selected):
                return []
            return selected
        return outputs

    def report(self):
        from reporting import controller_report
        return controller_report(self)

    def intent_deadline(self, row):
        return row['offered']+self.j.limits['start_slot_grace_seconds']

    def choose_ingress(self, identifier):
        """Seeded ingress; never a host the active degrade step names (#1587)."""
        if self.plan.get('schema_version') != 2:
            return HOSTS[0]
        live = self.live_hosts()
        return live[int(hashlib.sha256((str(self.plan['seed'])+identifier).encode()).hexdigest(),16)%len(live)]

    async def prepare(self, identifier, count, phase_limit, burst=False, crash=None, fee_multiplier=1, cohort=None, scan=None):
        row = self.j.intent(identifier)
        self.gate()
        if any(r['sender']==row['sender'] for r in self.j.pending()):
            return False
        # ``scan`` lets one admission pass share a single read-only sync
        # (#1618). Inputs signed earlier in the pass are already reserved,
        # and an older height only delays maturity, so reuse never widens
        # selection; every gate below still runs per intent.
        height = await (scan or self.sync)()
        selected = self.spendable(row['sender'],height,row['amount'],count)
        if not selected:
            self.j.event(identifier,'inventory_wait',{'wallet':row['sender'],'height':height,
                **self.j.get('selection_latest',{})})
            return False
        self.gate()
        if time.time() > self.intent_deadline(row):
            return False
        self.j.transition(identifier,'eligible')
        charged=self.j.db.execute('SELECT COALESCE(SUM(fee),0) FROM intents WHERE prepared IS NOT NULL').fetchone()[0]
        if charged+self.j.limits['max_single_signed_fee_picocredits']>self.j.limits['max_signed_fee_picocredits']:
            raise Gate('insufficient remaining fee budget to prepare')
        writes=self.j.db.execute('SELECT COUNT(*) FROM intents WHERE prepared IS NOT NULL OR submitted IS NOT NULL').fetchone()[0]
        if writes>=self.j.limits['max_unique_chain_writes']:
            raise Gate('chain write budget reached before signing')
        artifact = self.state/'artifacts'/(identifier+'.bin')
        if artifact.exists() or artifact.with_suffix('.partial').exists():
            raise Gate('orphaned prepared artifact needs reconciliation: '+identifier)
        ingress = self.choose_ingress(identifier)
        fee_rate = await self.rpc.call(ingress,'fee_getRate')
        blocks, spent = self.prepare_history(height, selected)
        info = await self.native({'operation':'prepare','wallet':self.wallets[row['sender']]['key'],
            'blocks':blocks,'height':height,'selected':[o['id'] for o in selected],
            'spent':spent,'reserved':[r[0] for r in self.j.db.execute('SELECT input FROM reservations')],
            'recipient':self.wallets[row['recipient']]['address'],
            'allowed_recipients':[w['address'] for w in self.wallets],
            'amount':row['amount'],'base_rate':int(fee_rate['baseRate']),'artifact':str(artifact),
            **({'fee_multiplier':fee_multiplier} if self.plan.get('schema_version') == 2 else {})})
        if self.plan.get('schema_version') == 2:
            from fee_evidence import signed_fee_evidence
            info.update(fee_evidence=signed_fee_evidence(info, fee_multiplier), cohort=cohort,
                        fee_quote=fee_rate, ingress_role=ingress)
        info.update(burst=burst,phase_limit=phase_limit,crash=crash,
                    scan_height=height,prepared_at=time.time())
        self.gate()
        self.j.prepare(identifier,info,phase_limit,self.intent_deadline(row))
        if crash=='prepared' and not self.j.get('crash:prepared'):
            self.j.set('crash:prepared',identifier)
            self.report()
            os._exit(77)
        await self.submit_prepared(identifier)
        return True

    async def submit_prepared(self, identifier):
        row = self.j.intent(identifier)
        info = json.loads(row['info'])
        self.gate()
        data = Path(info['artifact']).read_bytes()
        inspected = await self.native({'operation':'inspect','artifact':info['artifact']})
        for key in ('hash','fee','bytes','outputs','key_images'):
            if inspected[key] != info[key]:
                raise Gate('saved artifact does not match durable journal')
        if time.time() >= self.intent_deadline(row):
            self.j.transition(identifier,'expired','prepared intent exceeded original slot',finished=time.time())
            # Retain the signed input reservations: a signed artifact still exists.
            self.halt('prepared intent expired; review reserved inputs')
            return
        host = info.get('ingress_role', HOSTS[(sum(identifier.encode()) % len(HOSTS))])
        wire = len(json.dumps({'jsonrpc':'2.0','id':1404,'method':'tx_submit','params':{'tx_hex':data.hex()}}).encode())
        self.j.begin_submit(identifier,host,wire,info['burst'],self.intent_deadline(row))
        # Retain the durable marker's byte charge through receipt transitions.
        info = json.loads(self.j.intent(identifier)['info'])
        self.j.set('last_write',time.time())
        if info.get('crash')=='submitting' and not self.j.get('crash:submitting'):
            # Crash after the durable marker. No retry is possible on recovery;
            # this intentionally ambiguous intent must become unresolved if absent.
            # Exercised in isolated tests only, never selected by public schedule.
            self.j.set('crash:submitting',identifier)
            os._exit(77)
        try:
            result = await self.rpc.call(host,'tx_submit',{'tx_hex':data.hex()},timeout=30,write=True)
            if result.get('txHash') != row['hash']:
                raise Gate('submitted hash differs from canonical saved hash')
            self.j.transition(identifier,'accepted',info={**info,'response_at':time.time()})
            self.report()
            if info.get('crash')=='accepted' and not self.j.get('crash:accepted'):
                self.j.set('crash:accepted',identifier)
                self.report()
                os._exit(77)
        except Exception as error:
            self.j.transition(identifier,'unknown',str(error))
            self.halt('submission reply uncertain or rejected; retained input reservations')

    async def fund(self, identifier):
        row = self.j.intent(identifier)
        self.gate()
        if row['state'] != 'planned' or time.time()>self.intent_deadline(row):
            return
        faucet = await self.rpc.call('faucet.botho.io','faucet_getStatus')
        if not faucet.get('enabled') or int(faucet.get('amountPerRequest',0)) != 1_000_000_000_000:
            raise Gate('unexpected faucet configuration')
        self.gate()
        if time.time()>self.intent_deadline(row):
            return
        with self.j.transaction():
            rows = self.j.rows("kind='funding' AND submitted IS NOT NULL")
            if len(rows)>=24:
                raise Gate('funding count cap')
            # Fixed offer times can precede the previous actual submission + 900
            # seconds. Wait within this offer's original slot; never replay it.
            if any(r['submitted']>time.time()-900 for r in rows):
                return
            if sum(r['recipient']==row['recipient'] and r['submitted']>time.time()-86400 for r in rows)>=3:
                raise Gate('recipient daily funding cap')
            attempted = self.j.db.execute("SELECT COUNT(*) FROM intents WHERE submitted IS NOT NULL OR prepared IS NOT NULL").fetchone()[0]
            if attempted>=800:
                raise Gate('chain write budget')
            self.j.transition(identifier,'submitting',submitted=time.time(),ingress='faucet.botho.io')
        self.j.set('last_write',time.time())
        try:
            result = await self.rpc.call('faucet.botho.io','faucet_request',
                {'address':self.wallets[row['recipient']]['address']},timeout=90,write=True)
            if not result.get('success') or int(result.get('amount',0)) != row['amount'] or not re.fullmatch(r'[0-9a-f]{64}',result.get('txHash','')):
                raise Gate('faucet reply rejected or unexpected')
            self.j.transition(identifier,'accepted',hash=result['txHash'],info={'response_at':time.time()})
            self.report()
        except Exception as error:
            self.j.transition(identifier,'unknown',str(error))
            self.halt('faucet reply uncertain; funding slot consumed without retry')

    async def wait_for_reconcile(self, delay):
        try:
            await asyncio.wait_for(self.reconcile_wakeup.wait(), timeout=delay)
        except asyncio.TimeoutError:
            pass

    async def reconcile(self):
        next_poll = 0
        while not self.closed:
            # Height wakes only complete saved confirmations. Keep the ordinary
            # receipt/status polling deadline independent of monitor cadence.
            poll = time.monotonic() >= next_poll
            if poll:
                next_poll = time.monotonic()+30
            self.reconcile_wakeup.clear()
            try:
                if poll:
                    await self.verify_deferred()
                for row in self.j.pending():
                    # Admission alone owns first submission of prepared work.
                    # Quota pressure must not interrupt unrelated receipt reads.
                    if row['state']=='prepared':
                        continue
                    info = json.loads(row['info'])
                    saved = info.get('confirmed_receipts') if row['state']=='confirmed' else None
                    fetch_receipts = saved is None
                    if fetch_receipts:
                        if not poll:
                            continue
                        # Decision 6 (#1576): during a step a payment confirms
                        # on the live hosts; the named hosts are re-verified
                        # after window end (verify_deferred).
                        view = self.fault_view()
                        live = self.live_hosts(view=view)
                        if row['hash']:
                            statuses = await asyncio.gather(*(self.rpc.call(h,'getTransactionStatus',{'hash':row['hash']}) for h in live))
                        else:
                            statuses = []
                        now = time.time()
                        since = row['submitted'] or row['prepared']
                        # Only scheduled below-quorum time is excluded.
                        age = now-since-self.safe_halt_seconds(since,now)
                        if not statuses or not all(s.get('confirmed') and s.get('txHash')==row['hash'] for s in statuses):
                            if age>900:
                                self.j.transition(row['id'],'unresolved','not reconciled within fifteen minutes')
                                self.halt('unresolved payment exceeded fifteen minutes')
                            elif age>300:
                                self.halt('payment unconfirmed after five minutes')
                            continue
                        receipts = await asyncio.gather(*(self.rpc.call(h,'getTransaction',{'hash':row['hash']}) for h in live))
                        saved = dict(zip(live,receipts))
                        if len(live) != len(HOSTS):
                            info.update(deferred_hosts=[h for h in HOSTS if h not in live],
                                        deferred_step=view['step'])
                    deferred = info.get('deferred_hosts') or []
                    if not set(deferred) <= set(HOSTS) or set(saved) != set(HOSTS)-set(deferred):
                        raise Gate('incomplete saved confirmation receipts')
                    receipts = [saved[h] for h in HOSTS if h in saved]
                    if len({(r.get('blockHeight'),r.get('fee'),r.get('outputCount'),r.get('totalOutput')) for r in receipts}) != 1:
                        raise Gate('fleet transaction receipt disagreement')
                    if any(r.get('txHash',row['hash']) != row['hash'] for r in receipts):
                        raise Gate('fleet transaction receipt hash mismatch')
                    if fetch_receipts:
                        info.update(all_confirmed_at=time.time(),block_height=receipts[0]['blockHeight'],
                                    confirmed_receipts=saved)
                        self.j.transition(row['id'],'confirmed',info=info)
                    if info['block_height'] != receipts[0]['blockHeight']:
                        raise Gate('saved confirmation height mismatch')
                    # Monitor wakeups cannot authorize scans from stale or
                    # incomplete fleet state, including after a process restart.
                    current = [self.statuses[h] for h in self.live_hosts() if h in self.statuses]
                    if (set(self.statuses) != set(HOSTS) or time.time()-self.fresh > 60
                        or any(not s.get('synced') for s in current)
                        or min(s.get('chainHeight',-1) for s in current) < info['block_height']):
                        continue
                    await self.sync(draining=True)
                    if time.time()-self.fresh > 60:
                        raise Gate('fleet data stale during reconciliation')
                    recipient = self.inventory[row['recipient']]
                    received = [o for o in recipient if bytes(o['utxo']['tx_hash']).hex()==row['hash'] and o['utxo']['output_index']==0]
                    if len(received)!=1 or received[0]['utxo']['amount']!=row['amount']:
                        raise Gate('recipient ownership/value reconciliation failed')
                    if row['kind'] != 'funding':
                        if receipts[0]['fee'] != row['fee']:
                            raise Gate('node receipt fee differs from signed fee')
                        for expected in info['outputs']:
                            owner = row['recipient'] if expected['index']==0 else row['sender']
                            found = [o for o in self.inventory[owner] if bytes(o['utxo']['tx_hash']).hex()==row['hash']
                                     and o['utxo']['output_index']==expected['index']]
                            if len(found)!=1 or found[0]['utxo']['amount']!=expected['amount'] or bytes(found[0]['utxo']['target_key']).hex()!=expected['target_key']:
                                raise Gate('recipient/change metadata mismatch')
                        view = self.fault_view()
                        checked = self.live_hosts(view=view)
                        states = await asyncio.gather(*(self.rpc.call(h,'chain_areKeyImagesSpent',{'keyImages':info['key_images']}) for h in checked))
                        if any([s.get('keyImage') for s in results] != info['key_images'] or not all(s.get('spent') for s in results) for results in states):
                            raise Gate('spent-input state disagreement')
                        if len(checked) != len(HOSTS):
                            # Spent state not yet seen on the named hosts:
                            # they join the post-window re-verification.
                            missing = set(HOSTS)-set(checked)|set(info.get('deferred_hosts') or [])
                            info.update(deferred_hosts=[h for h in HOSTS if h in missing],
                                        deferred_step=view['step'])
                    else:
                        info['faucet_fee'] = receipts[0]['fee']
                    info['recipient_verified_at'] = time.time()
                    self.j.finish(row['id'],info)
                    self.report()
                if not self.j.pending() and self.blocks:
                    self.accounting()
                    self.report()
            except Quota as error:
                self.j.event(None,'reconcile_quota',str(error))
            except Exception as error:
                self.halt('reconciliation: '+str(error))
            if not self.closed:
                await self.wait_for_reconcile(max(.2,next_poll-time.monotonic()))

    async def verify_deferred(self):
        """Re-verify payments confirmed during a step on the hosts it named.

        Runs once the step's window has ended and while no later step names
        those hosts. Receipts and spent state must agree with the live hosts'
        saved evidence; that also covers rows already reconciled.
        """
        rows = self.deferred_rows()
        if not rows:
            return
        windows = {w['step']: w for w in self.degrade_windows()}
        now = time.time()
        view = self.fault_view(now)
        down = view['expected_down'] if view else ()
        for row in rows:
            info = json.loads(row['info'])
            hosts = info['deferred_hosts']
            window = windows.get(info.get('deferred_step'))
            if window is None or not hosts or not set(hosts) <= set(HOSTS):
                raise Gate('deferred confirmation names no scheduled step')
            if now < window['window_end'] or any(h in down for h in hosts):
                continue
            if time.time()-self.fresh > 60 or any(
                    self.statuses.get(h,{}).get('synced') is not True
                    or self.statuses[h].get('chainHeight',-1) < info['block_height'] for h in hosts):
                continue  # not caught up yet; the rejoin gate bounds this wait
            receipts = await asyncio.gather(*(self.rpc.call(h,'getTransaction',{'hash':row['hash']}) for h in hosts))
            saved = info['confirmed_receipts']
            shape = lambda r: (r.get('blockHeight'),r.get('fee'),r.get('outputCount'),r.get('totalOutput'))
            if len({shape(r) for r in [*saved.values(),*receipts]}) != 1:
                raise Gate('fleet transaction receipt disagreement')
            if any(r.get('txHash',row['hash']) != row['hash'] for r in receipts):
                raise Gate('fleet transaction receipt hash mismatch')
            if row['kind'] != 'funding':
                states = await asyncio.gather(*(self.rpc.call(h,'chain_areKeyImagesSpent',{'keyImages':info['key_images']}) for h in hosts))
                if any([s.get('keyImage') for s in results] != info['key_images'] or not all(s.get('spent') for s in results) for results in states):
                    raise Gate('spent-input state disagreement')
            saved.update(zip(hosts,receipts))
            del info['deferred_hosts']
            info['deferred_verified'] = {'step': window['step'], 'hosts': hosts, 'at': time.time()}
            with self.j.transaction():
                self.j.db.execute('UPDATE intents SET info=? WHERE id=?', (json.dumps(info), row['id']))
                self.j.event(row['id'],'deferred_verified',info['deferred_verified'])

    def accounting(self):
        state = {s['keyImage']:s for s in self.spent}
        if any(s['pending'] for s in self.spent):
            return
        balances = [sum(o['utxo']['amount'] for o in owned if not state[o['key_image']]['spent']) for owned in self.inventory]
        grants = sum(r['amount'] for r in self.j.rows("kind='funding' AND state='reconciled'"))
        fees = sum(r['fee'] for r in self.j.rows("kind!='funding' AND state='reconciled'"))
        # Recognize actual scanned lottery awards explicitly; never round a difference.
        lottery = {(o['txHash'],o['outputIndex']):int.from_bytes(bytes.fromhex(o['amountCommitment']),'little')
                   for b in self.blocks for o in b['outputs'] if o.get('lottery')}
        awards = sum(lottery.get((bytes(o['utxo']['tx_hash']).hex(),o['utxo']['output_index']),0)
                     for owned in self.inventory for o in owned)
        opening = self.j.get('opening_balance',0)
        report = {'at':time.time(),'opening':opening,'grants':grants,'lottery_receipts':awards,
                  'balances':balances,'ending':sum(balances),'signed_fees':fees,
                  'difference':opening+grants+awards-self.j.get('opening_lottery',0)-sum(balances)-fees,
                  'legacy_lottery':legacy_lottery_evidence(self.inventory,lottery,state)}
        self.j.set('accounting',report)
        if report['legacy_lottery']['award_count']:
            raise Gate('unsupported legacy lottery payouts share a key image; see accounting evidence')
        if report['difference'] != 0:
            raise Gate('exact integer accounting mismatch')

    async def setup_tick(self):
        start = self.j.get('setup_start')
        if time.time()>=start+8*3600:
            raise Gate('eight-hour setup deadline; inventory not ready')
        for index in range(24):
            due = start+index*900
            if time.time()<due:
                break
            identifier = 'funding-'+str(index).zfill(2)
            self.j.offer(identifier,'funding',due,None,index%8,1_000_000_000_000)
            row = self.j.intent(identifier)
            if row['state']=='planned':
                if time.time()>due+120:
                    self.j.transition(identifier,'skipped','funding slot missed',finished=time.time())
                elif not self.j.pending():
                    await self.fund(identifier)
                return
        if self.j.pending():
            return
        if time.time()-self.j.get('setup_scan_at',0)<30:
            return
        height = await self.sync()
        self.j.set('setup_scan_at',time.time())
        self.accounting()
        counts = [len(self.spendable(w,height)) for w in range(8)]
        self.j.set('inventory_counts',{'at':time.time(),'height':height,'mature_with_decoys':counts})
        if min(counts)>=4:
            selections = [self.spendable(w,height,10_000_000_000,4) for w in range(8)]
            if not all(selections):
                return
            height = await self.sync(full=True)
            self.accounting()
            # Recheck every complete selection at the refreshed height before
            # creating any protected probe artifact. Insufficient inventory is
            # a wait within the existing setup deadline, not a signer failure.
            selections = [self.spendable(w,height,10_000_000_000,4) for w in range(8)]
            if not all(selections):
                return
            # Readiness probes must use the actual signer, including every wallet's
            # four-input shape. Probe artifacts remain protected and never submitted.
            fee_rate = await self.rpc.call(HOSTS[0],'fee_getRate')
            for w in range(8):
                probe = self.state/'probes'/('wallet-'+str(w)+'.bin')
                if probe.exists():
                    raise Gate('setup probe already exists; operator review required')
                selected = selections[w]
                blocks, spent = self.prepare_history(height, selected)
                await self.native({'operation':'prepare','wallet':self.wallets[w]['key'],'blocks':blocks,
                    'height':height,'selected':[o['id'] for o in selected],'spent':spent,'reserved':[],
                    'recipient':self.wallets[(w+1)%8]['address'],'allowed_recipients':[x['address'] for x in self.wallets],
                    'amount':10_000_000_000,'base_rate':int(fee_rate['baseRate']),'artifact':str(probe)})
            self.gate()
            start = time.time()+60
            with self.j.transaction():
                self.j.set('start',start)
                self.j.set('end',start+72*3600)
                self.j.set('status','running')
                self.j.set('active_phase','correctness')
                self.j.set('phase_gates',{'correctness':True})
            atomic(self.state/'activated.json',{'run_id':self.config['run_id'],'plan_sha256':digest(self.plan),
                'start':start,'end':start+72*3600,'inventory':counts,'height':height})
            self.report()
            return
        bootstrap = self.j.rows("kind='bootstrap'")
        next_grant=start+(int((time.time()-start)//900)+1)*900
        if next_grant<=start+23*900 and next_grant-time.time()<240:
            return
        # Self-transfers split mature value while preserving all principal in the
        # allowlist. Start as soon as eligible funding outputs exist, interleaving
        # with the immutable 15-minute faucet schedule.
        for w in sorted(range(8), key=lambda i: counts[i]):
            if len(self.unspent_setup_inputs(w)) >= 4:
                # More splits consume fees and reset ages while these inputs
                # are waiting for maturity or age-matched decoys.
                continue
            if self.spendable(w, height, 100_000_000_000, 1):
                if len(bootstrap) >= 64:
                    raise Gate(
                        'setup inventory exhausted after sixty-four bootstrap offers'
                    )
                identifier = 'bootstrap-' + str(len(bootstrap)).zfill(2)
                self.j.offer(identifier,'bootstrap',time.time(),w,w,100_000_000_000)
                await self.prepare(identifier,1,1)
                return

    async def campaign_tick(self):
        start = self.j.get('start')
        now = time.time()
        if now >= self.j.get('end'):
            self.j.set('status','draining')
            self.j.set('stopped_at',self.j.get('end'))
            return
        elapsed = now-start
        # Materialize every offered slot before gates so downtime cannot hide
        # missing work at a phase boundary. Only eight live offers may queue.
        for event in self.events:
            offered=start+event['offset_seconds']
            if offered>now:
                break
            sender=int(event['sender'].split('-')[-1])-1
            recipient=int(event['recipient'].split('-')[-1])-1
            self.j.offer(event['intent'],'campaign',offered,sender,recipient,int(event['amount_picocredits']))
            row=self.j.intent(event['intent'])
            if row['state'] in ('planned','eligible') and now>offered+120:
                self.j.transition(event['intent'],'skipped','slot expired; no catch-up',finished=now)
        queued=self.j.rows("kind='campaign' AND state IN ('planned','eligible')")
        for row in queued[8:]:
            self.j.transition(row['id'],'skipped','queue budget',finished=now)
        phase = next((p for p in self.plan['phases'] if p['start_hour']*3600 <= elapsed < p['end_hour']*3600),None)
        if not phase:
            return
        active = self.j.get('active_phase')
        if phase['id'] != active:
            earlier = self.j.rows("kind='campaign' AND offered<?",(start+phase['start_hour']*3600,))
            if self.j.pending() or any(r['state']!='reconciled' for r in earlier):
                raise Gate('previous phase incomplete; escalation held')
            await self.sync(full=True)
            self.accounting()
            if active=='correctness':
                shapes={json.loads(r['info']).get('input_count') for r in earlier}
                if not {1,2,4} <= shapes:
                    raise Gate('correctness input-shape coverage missing')
            counts=[len(self.spendable(w,min(s['chainHeight'] for s in self.statuses.values()))) for w in range(8)]
            if min(counts)<1:
                raise Gate('inventory_exhausted at phase boundary')
            self.j.set('active_phase',phase['id'])
            gates=self.j.get('phase_gates')
            gates[phase['id']]=True
            self.j.set('phase_gates',gates)
            self.report()
        for event in self.events:
            offered=start+event['offset_seconds']
            if offered>now:
                break
            sender=int(event['sender'].split('-')[-1])-1
            recipient=int(event['recipient'].split('-')[-1])-1
            self.j.offer(event['intent'],'campaign',offered,sender,recipient,int(event['amount_picocredits']))
            row=self.j.intent(event['intent'])
            if row['state'] not in ('planned','eligible'):
                continue
            if now>offered+120:
                self.j.transition(event['intent'],'skipped','slot expired; no catch-up',finished=now)
                continue
            # Confirmed-but-unscanned rows do not occupy the cap; prepare()
            # still refuses their senders via pending() (#1551).
            if len(self.j.inflight())>=event['max_inflight']:
                continue
            index=int(event['intent'].split('-')[-1])
            count = (2 if index==2 else 4 if index==4 else 1)
            if index==6 or (phase['id']=='recovery' and not self.j.get('restore_recovery')):
                await self.restore_wallet(sender)
                if phase['id']=='recovery':
                    self.j.set('restore_recovery',True)
            crash = None
            if phase['id']=='recovery':
                if not self.j.get('crash:prepared'):
                    crash='prepared'
                elif not self.j.get('crash:accepted'):
                    crash='accepted'
            await self.prepare(event['intent'],count,event['max_inflight'],event['scenario']=='burst',crash)
            # Offers remain fixed. A slow signer must not turn expired slots into
            # a backlog; the next tick records all missed offers explicitly.
            return

    async def restore_wallet(self,wallet):
        if any(r['sender']==wallet for r in self.j.pending()):
            raise Gate('cannot hand off a wallet with unresolved inputs')
        old=Path(self.wallets[wallet]['key'])
        restored=self.state/'wallets'/('restored-'+str(wallet)+'.mnemonic')
        if not restored.exists():
            with old.open('rb') as source, restored.open('xb') as target:
                shutil.copyfileobj(source,target)
                target.flush()
                os.fsync(target.fileno())
        # Serial on purpose: both are whole-chain scans, and the controller
        # unit's memory budget does not hold two at once (#1629).
        before=await self.native({'operation':'scan','wallet':str(old),'blocks':self.blocks})
        after=await self.native({'operation':'restore_check','wallet':str(restored),'blocks':self.blocks,
                                'expected_address':self.wallets[wallet]['address']})
        if before!=after:
            raise Gate('restored wallet inventory differs')
        self.wallets[wallet]['key']=str(restored)
        self.j.set('wallet_paths',[w['key'] for w in self.wallets])
        self.j.event(None,'wallet_restored',{'wallet':wallet})

    async def admission_tick(self):
        async with self.admission_lock:
            # One tick may admit several offers (#1618); the v2 deliver pass
            # measures its wall-clock budget from here.
            self.admission_started = time.time()
            self.gate()
            waiting = self.j.rows("state='prepared' ORDER BY prepared, offered, id")
            for row in waiting:
                # Persisted signatures have no submit marker. Require every
                # ingress to report unknown before their one allowed attempt.
                # A named host is skipped; the recorded ingress is not
                # re-routed (submit_prepared), so a down ingress still halts.
                statuses = await asyncio.gather(*(self.rpc.call(
                    h,'getTransactionStatus',{'hash':row['hash']}) for h in self.live_hosts()))
                if any(s.get('status')!='unknown' for s in statuses):
                    self.halt('unsubmitted prepared hash unexpectedly exists on chain')
                    return
                # A Quota deferral propagates out of this tick: the signature
                # keeps its slot and nothing newer is submitted or signed.
                await self.submit_prepared(row['id'])
                if self.j.get('status') not in ('setup','running'):
                    return
            if waiting:
                # Even when quota defers this signature, never let new signing
                # steal its next capacity slot or reserve more wallet inputs.
                # Waiting rows mean quota (or a crash) just bound, so new
                # signing resumes on the next tick, not this one.
                return
            if self.j.get('status')=='setup':
                await self.setup_tick()
            else:
                await self.campaign_tick()

    async def run(self):
        if not self.j.get('status'):
            self.j.set('setup_start',self.config['setup_start'])
            self.j.set('status','setup')
        paths=self.j.get('wallet_paths')
        if paths:
            for wallet,path in zip(self.wallets,paths):
                wallet['key']=path
        # A crashed signing operation may have left a published artifact before
        # SQLite preparation. Hold rather than sign again or silently reuse it.
        for artifact in (self.state/'artifacts').iterdir():
            row=self.j.intent(artifact.stem)
            if artifact.suffix!='.bin' or not row or not row['prepared']:
                self.halt('orphaned or partial signed artifact after restart')
        monitor=asyncio.create_task(self.monitor())
        reconciler=None
        try:
            startup=time.time()
            while not self.fresh and time.time()-startup<55 and self.j.get('status') in ('setup','running'):
                await asyncio.sleep(1)
            if self.fresh:
                await self.sync(draining=True)
            reconciler=asyncio.create_task(self.reconcile())
            while True:
                started=time.monotonic()
                status=self.j.get('status')
                if status in ('complete','incomplete','rehearsal_complete'):
                    break
                if status=='running' and time.time()>=self.j.get('end'):
                    self.j.set('status','draining')
                    self.j.set('stopped_at',self.j.get('end'))
                    status='draining'
                if (self.state/'STOP').exists():
                    self.halt('operator stop marker')
                if status in ('held','draining') and time.time()>self.j.get('stopped_at',time.time())+1800:
                    break
                try:
                    if status in ('setup','running'):
                        await self.admission_tick()
                except Quota as error:
                    self.j.event(None,'admission_quota',str(error))
                except Exception as error:
                    self.halt(str(error))
                if time.time()-self.j.get('last_summary_at',0)>21600:
                    self.report()
                    atomic(self.state/'reports'/str(int(time.time())),self.report())
                    self.j.set('last_summary_at',time.time())
                # Include admission/report work in the cadence, but always
                # yield so monitoring and reconciliation can make progress.
                await asyncio.sleep(max(.2,1-(time.monotonic()-started)))
            if self.j.get('status') == 'rehearsal_complete':
                return
            if self.fresh and not self.j.pending():
                await self.sync(full=True, draining=True)
                self.accounting()
            try:
                self.settle_degrade_verdicts()
            except Gate as error:
                self.j.event(None,'degrade_verdict_failed',str(error))
            from reporting import workload_complete
            success=workload_complete(self.j,self.events,self.plan['phases'],
                                      getattr(self,'degrade_steps',None) or [])
            self.j.set('status','complete' if success else 'incomplete')
        finally:
            self.closed=True
            monitor.cancel()
            if reconciler:
                reconciler.cancel()
            self.report()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode',choices=['run','report'])
    parser.add_argument('--state',type=Path,required=True)
    args=parser.parse_args()
    os.umask(0o077)
    with (args.state/'controller.lock').open('a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        controller_type = Controller
        if json.loads((args.state/'plan.json').read_text()).get('schema_version') == 2:
            from discovery import DiscoveryController
            controller_type = DiscoveryController
        controller=controller_type(args.state)
        if args.mode=='report':
            print(json.dumps(controller.report(),indent=2))
        else:
            asyncio.run(controller.run())


if __name__=='__main__':
    main()
