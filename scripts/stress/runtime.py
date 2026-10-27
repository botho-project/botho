#!/usr/bin/env python3
"""Durable primitives for the bounded testnet campaign. Python standard library only."""
import asyncio
import contextlib
import email.utils
import hashlib
import json
import math
import os
import re
import ssl
from pathlib import Path
import sqlite3
import time
import urllib.error
import urllib.request
import urllib.parse

HOSTS = ('seed.botho.io', 'seed2.botho.io', 'faucet.botho.io',
         'eu.seed.botho.io', 'ap.seed.botho.io')
# Explicit-mode threshold for the five x-series validators: 4-of-5, f = 1
# (#1576). Defined here, not in fleet_nodes, because the controller host only
# receives staging.CODE: fleet_nodes re-exports it to render configs, and
# check_quorum reads it to verify the installed quorum (#1605).
QUORUM_THRESHOLD = 4
TERMINAL = ('reconciled', 'skipped', 'rejected', 'expired', 'aborted')
PENDING = ('prepared', 'submitting', 'accepted', 'unknown', 'confirmed', 'recipient_verified')
# The subset of PENDING still occupying chain/ingress capacity: not yet
# confirmed on every host. Confirmed rows awaiting the batched wallet scan
# (recipient/change verification) stay PENDING -- they still reserve their
# sender, gate phase boundaries and drain -- but no longer count against
# max_inflight, so a burst is not throttled by the reconcile cadence (#1551).
INFLIGHT = ('prepared', 'submitting', 'accepted', 'unknown')
assert set(INFLIGHT) < set(PENDING)


class Gate(Exception):
    """A fail-closed admission decision, retained for explicit operator review."""


def validate_rpc_url(value):
    """A canonical HTTPS RPC destination; no URL credentials or extra routing."""
    if not isinstance(value, str) or not re.fullmatch(
            r'https://[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[1-9][0-9]{0,4})?/rpc', value):
        raise Gate('invalid HTTPS RPC URL')
    parsed = urllib.parse.urlsplit(value)
    try:
        port = parsed.port
    except ValueError as error:
        raise Gate('invalid RPC port') from error
    if port is not None and not 1 <= port <= 65535:
        raise Gate('invalid RPC port')
    return value


def validate_ssh_target(value):
    if not isinstance(value, str) or not re.fullmatch(
            r'[a-z_][a-z0-9_-]*@[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?', value):
        raise Gate('invalid explicit user@host SSH target')
    return value


def campaign_targets(config, plan):
    """Legacy launches stay bound to legacy endpoints; new maps are all-or-nothing."""
    legacy = [{'role': h, 'rpc_url': 'https://'+h+'/rpc',
               'observer_ssh_target': 'ubuntu@'+h} for h in HOSTS]
    rows = config.get('targets', legacy)
    if not isinstance(rows, list) or len(rows) != len(HOSTS):
        raise Gate('complete five-target profile required')
    if any(not isinstance(row, dict) or set(row) != {'role','rpc_url','observer_ssh_target'}
           for row in rows):
        raise Gate('target fields must be explicit')
    if [row['role'] for row in rows] != list(HOSTS):
        raise Gate('target roles must match the fixed logical role order')
    urls = [validate_rpc_url(row['rpc_url']) for row in rows]
    ssh = [validate_ssh_target(row['observer_ssh_target']) for row in rows]
    if len(set(urls)) != 5 or len(set(ssh)) != 5 or urls != plan.get('endpoints'):
        raise Gate('five distinct targets must exactly match plan endpoints in order')
    return {row['role']: dict(row) for row in rows}


def pinned_file(path, expected, name):
    if not isinstance(path, str) or not Path(path).is_absolute() or not re.fullmatch(r'[0-9a-f]{64}', str(expected)):
        raise Gate(name+' requires an absolute path and SHA256 pin')
    if hashlib.sha256(Path(path).read_bytes()).hexdigest() != expected:
        raise Gate(name+' differs from launch pin')
    return path


def campaign_tls_context(config):
    has_file, has_pin = 'tls_ca_file' in config, 'tls_ca_sha256' in config
    if has_file != has_pin:
        raise Gate('private CA requires both file and SHA256 pin')
    cafile = pinned_file(config['tls_ca_file'],config['tls_ca_sha256'],'TLS CA') if has_file else None
    return ssl.create_default_context(cafile=cafile)


class Quota(Exception):
    pass


def fsync_directory(path):
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic(path, value):
    path = Path(path)
    tmp = path.with_suffix(path.suffix + '.tmp')
    with tmp.open('w') as output:
        json.dump(value, output, sort_keys=True)
        output.write('\n')
        output.flush()
        os.fsync(output.fileno())
    os.replace(tmp, path)
    fsync_directory(path.parent)


def atomic_stream(path, chunks):
    """``atomic()`` for a file produced as text ``chunks`` (#1633).

    Same temporary file, fsync, rename and directory fsync, so readers see
    the old file or the complete new one. The caller never holds the whole
    serialized value: each chunk is written and dropped.
    """
    path = Path(path)
    tmp = path.with_suffix(path.suffix + '.tmp')
    with tmp.open('w') as output:
        for chunk in chunks:
            output.write(chunk)
        output.flush()
        os.fsync(output.fileno())
    os.replace(tmp, path)
    fsync_directory(path.parent)


def _dumps(value):
    return json.dumps(value, sort_keys=True)


def inventory_chunks(at, height, owned, spent):
    """``atomic(path, {'at', 'height', 'owned', 'spent'})``'s exact bytes, in chunks.

    One chunk per wallet and per 4,096 spent rows, each encoded by the C
    encoder (``json.dump`` to a file uses the pure-Python one, which cost
    seconds per sync at x-series size, #1633).
    """
    yield '{"at": ' + _dumps(at) + ', "height": ' + _dumps(height) + ', "owned": ['
    for index, outputs in enumerate(owned):
        yield (', [' if index else '[') + ', '.join(map(_dumps, outputs)) + ']'
    yield '], "spent": ['
    for first in range(0, len(spent), 4096):
        yield (', ' if first else '') + ', '.join(map(_dumps, spent[first:first + 4096]))
    yield ']}\n'


class BlockLog:
    """Append-only output-history checkpoint, one block per line (#1633).

    Replaces rewriting the whole ``blocks.json`` on every sync. The file is
    only ever (a) appended to, after the sync that fetched the blocks has
    validated them, or (b) atomically replaced by the complete history.
    A crash during an append can leave only whole validated lines plus at
    most one torn final line (no newline). ``load`` drops that torn tail
    and the next append truncates it. Every whole line must parse and carry
    the next height (0, 1, 2, ...), else the log is corrupt and ``load``
    fails closed, exactly as an unparsable ``blocks.json`` did.

    ``persisted`` is the in-memory history list the file is known to equal.
    An append is used only when the caller extends exactly that list.
    Otherwise (first write, a legacy ``blocks.json`` load, or a caller that
    replaced the list) the whole history is rewritten atomically.
    """

    NAME = 'blocks.jsonl'
    LEGACY = 'blocks.json'

    def __init__(self, state):
        self.path = Path(state) / self.NAME
        self.legacy = Path(state) / self.LEGACY
        self.persisted = None
        self.size = None

    def load(self):
        """The checkpointed history: the log, else a legacy ``blocks.json``, else []."""
        if not self.path.exists():
            blocks = json.loads(self.legacy.read_text()) if self.legacy.exists() else []
            # Converted to the log by the first persist (a full rewrite).
            self.persisted, self.size = None, None
            return blocks
        blocks, size = [], 0
        with self.path.open('rb') as log:
            for number, line in enumerate(log):
                if not line.endswith(b'\n'):
                    break  # torn final append: never part of the checkpoint
                try:
                    block = json.loads(line)
                except ValueError:
                    raise Gate('block log corrupt at line %d' % (number + 1)) from None
                if not isinstance(block, dict) or block.get('height') != len(blocks):
                    raise Gate('block log corrupt at line %d' % (number + 1))
                blocks.append(block)
                size += len(line)
        self.persisted, self.size = blocks, size
        return blocks

    def persist(self, previous, blocks):
        """Make the file equal ``blocks``, which extends ``previous`` (its prefix)."""
        if self.persisted is not None and self.persisted is previous and self.size is not None:
            appended = blocks[len(previous):]
            if appended:
                with self.path.open('r+b') as log:
                    # Discard a torn tail an earlier failed append left behind.
                    log.truncate(self.size)
                    log.seek(self.size)
                    for block in appended:
                        log.write((_dumps(block) + '\n').encode())
                    log.flush()
                    os.fsync(log.fileno())
                    size = log.tell()
                self.size = size
        else:
            # Unknown file state: replace it with the whole history at once.
            self.persisted = self.size = None
            atomic_stream(self.path, (_dumps(block) + '\n' for block in blocks))
            self.size = self.path.stat().st_size
        self.persisted = blocks


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


class Journal:
    def __init__(self, path, clock=time.time, limits=None):
        from bounds import LEGACY, checked_limits
        self.limits = checked_limits(limits) if limits is not None else dict(LEGACY)
        self.clock = clock
        self.db = sqlite3.connect(path, isolation_level=None)
        self.db.row_factory = sqlite3.Row
        self.db.execute('PRAGMA journal_mode=WAL')
        self.db.execute('PRAGMA synchronous=FULL')
        self.db.executescript('''
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS intents(
                id TEXT PRIMARY KEY, kind TEXT NOT NULL, offered REAL NOT NULL,
                state TEXT NOT NULL, sender INTEGER, recipient INTEGER NOT NULL,
                amount INTEGER NOT NULL, hash TEXT UNIQUE, fee INTEGER NOT NULL DEFAULT 0,
                prepared REAL, submitted REAL, finished REAL, ingress TEXT,
                info TEXT NOT NULL DEFAULT '{}', reason TEXT);
            CREATE TABLE IF NOT EXISTS reservations(
                input TEXT PRIMARY KEY, intent TEXT NOT NULL, wallet INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS events(at REAL NOT NULL, intent TEXT, state TEXT, detail TEXT);
            CREATE TABLE IF NOT EXISTS requests(
                id INTEGER PRIMARY KEY, at REAL NOT NULL, host TEXT NOT NULL,
                method TEXT NOT NULL, bytes INTEGER NOT NULL, latency REAL, status TEXT);
            CREATE INDEX IF NOT EXISTS requests_window ON requests(host,at);
        ''')
        if limits is not None:
            existing = self.get('limits_digest')
            if existing is None and (self.db.execute('SELECT COUNT(*) FROM intents').fetchone()[0] or self.get('plan_digest') is not None):
                self.db.close()
                raise Gate('cannot migrate historical journal to new limits')
            if existing not in (None, digest(self.limits)):
                self.db.close()
                raise Gate('journal limits changed')
            self.set('limits_digest', digest(self.limits))

    def get(self, key, default=None):
        row = self.db.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone()
        return json.loads(row[0]) if row else default

    def set(self, key, value):
        self.db.execute('INSERT OR REPLACE INTO meta VALUES(?,?)', (key, json.dumps(value)))

    @contextlib.contextmanager
    def transaction(self):
        self.db.execute('BEGIN IMMEDIATE')
        try:
            yield
            self.db.execute('COMMIT')
        except BaseException:
            self.db.execute('ROLLBACK')
            raise

    def event(self, intent, state, detail):
        self.db.execute('INSERT INTO events VALUES(?,?,?,?)',
                        (self.clock(), intent, state, json.dumps(detail)))

    def intent(self, identifier):
        row = self.db.execute('SELECT * FROM intents WHERE id=?', (identifier,)).fetchone()
        return dict(row) if row else None

    def rows(self, where='1', args=()):
        return [dict(r) for r in self.db.execute('SELECT * FROM intents WHERE ' + where, args)]

    def offer(self, identifier, kind, at, sender, recipient, amount):
        self.db.execute('INSERT OR IGNORE INTO intents(id,kind,offered,state,sender,recipient,amount) '
                        'VALUES(?,?,?,?,?,?,?)', (identifier, kind, at, 'planned', sender, recipient, amount))

    def transition(self, identifier, state, reason=None, **fields):
        allowed = {'hash', 'fee', 'prepared', 'submitted', 'finished', 'ingress', 'info'}
        if not set(fields) <= allowed:
            raise ValueError('invalid journal field')
        if 'info' in fields:
            fields['info'] = json.dumps(fields['info'])
        fields.update(state=state, reason=reason)
        self.db.execute('UPDATE intents SET ' + ','.join(k+'=?' for k in fields) + ' WHERE id=?',
                        (*fields.values(), identifier))
        self.event(identifier, state, reason or {})

    def pending(self):
        return self.rows('state IN (' + ','.join('?' for _ in PENDING) + ')', PENDING)

    def inflight(self):
        """Rows counted against max_inflight; see INFLIGHT. Sender reuse and
        every other safety check must keep using pending()."""
        return self.rows('state IN (' + ','.join('?' for _ in INFLIGHT) + ')', INFLIGHT)

    def prepare(self, identifier, artifact, phase_limit, deadline):
        """Artifact is already fsynced. Reserve every input and actual fee atomically."""
        with self.transaction():
            intent = self.intent(identifier)
            if intent['state'] != 'eligible' or self.clock() >= deadline:
                raise Gate('preparation expired or intent already consumed')
            pending = self.pending()
            if len(self.inflight()) >= min(self.limits['max_inflight'], phase_limit):
                raise Gate('inflight limit')
            if any(r['sender'] == intent['sender'] for r in pending if r['sender'] is not None):
                raise Gate('wallet already reserved')
            attempted = self.db.execute("SELECT COUNT(*) FROM intents WHERE prepared IS NOT NULL OR submitted IS NOT NULL").fetchone()[0]
            if attempted >= self.limits['max_unique_chain_writes']:
                raise Gate('chain write budget')
            fees = self.db.execute('SELECT COALESCE(SUM(fee),0) FROM intents WHERE prepared IS NOT NULL').fetchone()[0]
            if not 0 < artifact['fee'] <= self.limits['max_single_signed_fee_picocredits'] or fees + artifact['fee'] > self.limits['max_signed_fee_picocredits']:
                raise Gate('signed fee budget')
            if artifact['bytes'] > self.limits['max_signed_transaction_bytes']:
                raise Gate('signed byte budget')
            if not artifact['selected'] or len({item['id'] for item in artifact['selected']}) != len(artifact['selected']):
                raise Gate('duplicate or missing selected inputs')
            for item in artifact['selected']:
                self.db.execute('INSERT INTO reservations VALUES(?,?,?)',
                                (item['id'], identifier, intent['sender']))
            self.transition(identifier, 'prepared', hash=artifact['hash'], fee=artifact['fee'],
                            prepared=self.clock(), info=artifact)

    def begin_submit(self, identifier, host, wire_bytes, burst, deadline):
        with self.transaction():
            row = self.intent(identifier)
            if row['state'] != 'prepared' or row['submitted'] is not None:
                raise Gate('submission marker already consumed')
            if self.clock() >= deadline:
                raise Gate('submission deadline')
            recent = self.rows('submitted > ?', (self.clock()-60,))
            if len(recent) >= (self.limits['max_burst_size'] if burst else self.limits['max_normal_submissions_per_minute']):
                raise Quota('submission rate budget')
            recent_bytes = sum(json.loads(r['info']).get('wire_bytes', 0) for r in recent)
            if recent_bytes + wire_bytes > self.limits['max_wire_submission_bytes_per_minute']:
                raise Quota('wire byte budget')
            info = json.loads(row['info'])
            info['wire_bytes'] = wire_bytes
            self.transition(identifier, 'submitting', submitted=self.clock(), ingress=host, info=info)

    def finish(self, identifier, info):
        with self.transaction():
            self.transition(identifier, 'reconciled', finished=self.clock(), info=info)
            self.db.execute('DELETE FROM reservations WHERE intent=?', (identifier,))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise Gate('RPC redirect is outside the endpoint allowlist')


class Rpc:
    def __init__(self, journal, targets=None, tls_context=None, allow_env_proxy=True):
        self.journal = journal
        self.targets = targets or campaign_targets({}, {"endpoints": ["https://"+h+"/rpc" for h in HOSTS]})
        self.locks = {h: asyncio.Semaphore(2) for h in HOSTS}
        self.opener = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPSHandler(context=tls_context),
            *([] if allow_env_proxy else [urllib.request.ProxyHandler({})]))

    def reserve(self, host, method, bytes_count):
        if host not in HOSTS:
            raise Gate('unlisted RPC host')
        now = self.journal.clock()
        if now < self.journal.get('backoff:'+host, 0):
            raise Quota('endpoint Retry-After')
        count = self.journal.db.execute('SELECT COUNT(*) FROM requests WHERE host=? AND at>?',
                                        (host, now-60)).fetchone()[0]
        if count >= min(self.journal.limits['max_rpc_per_endpoint_per_minute'], self.journal.get('quota:'+host, 100)//2):
            raise Quota('shared endpoint request budget')
        return self.journal.db.execute('INSERT INTO requests(at,host,method,bytes) VALUES(?,?,?,?)',
                                      (now, host, method, bytes_count)).lastrowid

    async def metrics(self, host, timeout=10):
        """Loop-stall counters from the host's metrics server (#1591); see read_metrics."""
        return await read_metrics(self, host, timeout)

    async def call(self, host, method, params=None, timeout=10, write=False):
        payload = json.dumps({'jsonrpc':'2.0', 'id':1404, 'method':method, 'params':params or {}}).encode()
        async with self.locks[host]:
            request_id = self.reserve(host, method, len(payload))
            start = time.monotonic()
            def send():
                request = urllib.request.Request(self.targets[host]['rpc_url'], payload,
                                                 {'Content-Type':'application/json'})
                with self.opener.open(request, timeout=timeout) as response:
                    data = response.read(8*1024*1024+1)
                    if len(data) > 8*1024*1024:
                        raise Gate('RPC response byte bound')
                    return json.loads(data), dict(response.headers)
            status = 'unknown'
            try:
                result, headers = await asyncio.to_thread(send)
                quota = headers.get('X-RateLimit-Limit') or headers.get('x-ratelimit-limit')
                if quota and quota.isdecimal():
                    self.journal.set('quota:'+host, int(quota))
                if result.get('id') != 1404 or result.get('jsonrpc') != '2.0':
                    raise Gate('RPC response identity mismatch')
                if result.get('error') is not None:
                    status = 'rpc_error'
                    raise Gate('RPC '+method+': '+str(result['error']))
                if 'result' not in result:
                    raise Gate('RPC result missing')
                status = 'ok'
                return result['result']
            except urllib.error.HTTPError as error:
                status = str(error.code)
                if error.code == 429:
                    retry = error.headers.get('Retry-After', '60')
                    if retry.isdecimal():
                        delay = int(retry)
                    else:
                        try:
                            delay = max(0,email.utils.parsedate_to_datetime(retry).timestamp()-self.journal.clock())
                        except (TypeError,ValueError):
                            delay = 60
                    self.journal.set('backoff:'+host, self.journal.clock()+max(60,delay))
                    raise Quota('HTTP 429; no automatic write retry') from error
                raise
            finally:
                self.journal.db.execute('UPDATE requests SET latency=?,status=? WHERE id=?',
                    (time.monotonic()-start,status,request_id))


# Loop-stall families served only by the node's standalone metrics server
# (`metrics_port`, the global REGISTRY), never by the RPC listener's /metrics
# (#1591). #1589 proxies `https://<host>/metrics` to that server.
LOOP_BUCKET = 'botho_node_loop_iteration_seconds_bucket'
LOOP_DELAYED = 'botho_node_loop_delayed_total'
LOOP_KEYS = ('iterations', 'within_1s', 'delayed')
METRICS_BYTES = 8*1024*1024
_SAMPLE = re.compile(r'([a-zA-Z_:][a-zA-Z0-9_:]*)(\{[^}]*\})?[ \t]+(\S+)(?:[ \t]+\S+)?')


def _metric_count(value):
    try:
        number = float(value)
    except ValueError as error:
        raise Gate('malformed loop metric value') from error
    if not math.isfinite(number) or number < 0 or not number.is_integer():
        raise Gate('malformed loop metric value')
    return int(number)


def parse_loop_metrics(text):
    """The two loop families from a metrics-server exposition.

    Returns cumulative ``iterations`` (the ``le="+Inf"`` bucket),
    ``within_1s`` (the ``le="1"`` bucket; the Rust encoder prints 1.0 as
    "1") and ``delayed`` (iterations >= 5 s). Every other family is ignored.
    """
    buckets, delayed = {}, None
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        match = _SAMPLE.fullmatch(line)
        if not match or match.group(1) not in (LOOP_BUCKET, LOOP_DELAYED):
            continue
        name, labels, value = match.groups()
        if name == LOOP_DELAYED:
            if delayed is not None:
                raise Gate('duplicate loop metric sample')
            delayed = _metric_count(value)
            continue
        bound = re.search(r'(?:^|[{,])le="([^"]*)"', labels or '')
        if not bound:
            raise Gate('malformed loop histogram bucket')
        try:
            upper = float(bound.group(1))
        except ValueError as error:
            raise Gate('malformed loop histogram bucket') from error
        if upper in buckets:
            raise Gate('duplicate loop metric sample')
        buckets[upper] = _metric_count(value)
    if delayed is None or math.inf not in buckets or 1.0 not in buckets:
        raise Gate('loop metric families missing from metrics server')
    if buckets[1.0] > buckets[math.inf]:
        raise Gate('malformed loop histogram bucket')
    return {'iterations': buckets[math.inf], 'within_1s': buckets[1.0], 'delayed': delayed}


def loop_delta(before, after, restarted=False):
    """Loop-stall counts accrued from ``before`` to ``after``.

    A node restart resets the counters: ``restarted`` (the step re-baselined
    this host), a missing ``before``, or any counter going backwards all count
    from zero up to ``after``. ``over_1s`` is the +Inf delta minus the
    ``le="1"`` delta; ``delayed`` is the >= 5 s counter delta.
    """
    reset = restarted or before is None or any(after[k] < before[k] for k in LOOP_KEYS)
    base = dict.fromkeys(LOOP_KEYS, 0) if reset else before
    delta = {k: after[k]-base[k] for k in LOOP_KEYS}
    over = delta['iterations']-delta['within_1s']
    if over < 0:
        raise Gate('malformed loop histogram delta')
    return {'over_1s': over, 'delayed': delta['delayed'], 'reset': reset}


def _http_backoff(journal, host, error):
    retry = error.headers.get('Retry-After', '60')
    if retry.isdecimal():
        delay = int(retry)
    else:
        try:
            delay = max(0,email.utils.parsedate_to_datetime(retry).timestamp()-journal.clock())
        except (TypeError,ValueError):
            delay = 60
    journal.set('backoff:'+host, journal.clock()+max(60,delay))


async def read_metrics(rpc, host, timeout=10):
    """Read-only GET of ``<rpc_url minus /rpc>/metrics`` for ``rpc`` (an Rpc).

    Same TLS context, redirect refusal, shared endpoint quota (method
    ``metrics``) and 8 MiB bound as ``Rpc.call``.
    """
    if host not in HOSTS:
        raise Gate('unlisted RPC host')
    url = rpc.targets[host]['rpc_url']
    if not url.endswith('/rpc'):
        raise Gate('invalid HTTPS RPC URL')
    url = url[:-len('/rpc')]+'/metrics'
    async with rpc.locks[host]:
        request_id = rpc.reserve(host, 'metrics', 0)
        start = time.monotonic()
        def fetch():
            request = urllib.request.Request(url, headers={'Accept': 'text/plain'})
            with rpc.opener.open(request, timeout=timeout) as response:
                data = response.read(METRICS_BYTES+1)
                if len(data) > METRICS_BYTES:
                    raise Gate('metrics response byte bound')
                return data.decode('utf-8', 'replace'), dict(response.headers)
        status = 'unknown'
        try:
            text, headers = await asyncio.to_thread(fetch)
            quota = headers.get('X-RateLimit-Limit') or headers.get('x-ratelimit-limit')
            if quota and quota.isdecimal():
                rpc.journal.set('quota:'+host, int(quota))
            parsed = parse_loop_metrics(text)
            status = 'ok'
            return parsed
        except urllib.error.HTTPError as error:
            status = str(error.code)
            if error.code == 429:
                _http_backoff(rpc.journal, host, error)
                raise Quota('HTTP 429; metrics read backs off') from error
            raise
        finally:
            rpc.journal.db.execute('UPDATE requests SET latency=?,status=? WHERE id=?',
                (time.monotonic()-start,status,request_id))


def check_identity(status, plan):
    if (status.get('network') != plan['network'] or status.get('gitCommit') != plan['node_commit']
            or status.get('version') != '0.6.0'):
        raise Gate('network or software identity changed')


def byzantine_tolerance(n, t):
    """f = min(n - t, 2t - n - 1), saturating; mirrors botho::config::byzantine_tolerance."""
    return max(0, min(n - t, 2 * t - n - 1))


def expected_quorum():
    """(n, t, f) every x-series validator must report: 4-of-5 explicit, f = 1.

    Derived from QUORUM_THRESHOLD (which fleet_nodes renders) and len(HOSTS)
    rather than restated.
    """
    n = len(HOSTS)
    return n, QUORUM_THRESHOLD, byzantine_tolerance(n, QUORUM_THRESHOLD)


# Config-derived quorum fields are non-null from node start; the gate-derived
# ones are all null until the gate's first evaluation (#1608).
QUORUM_CONFIG_KEYS = ('quorumMode', 'quorumFaultTolerant', 'quorumDegenerate')
QUORUM_GATE_KEYS = ('quorumSize', 'quorumThreshold', 'quorumByzantineTolerance',
                    'quorumAutoMembers', 'quorumGateIntersectionRefused')


def check_quorum(status, allow_unevaluated=False):
    """Fail closed unless the node reports the installed explicit 4-of-5, f = 1 quorum (#1605).

    The node drops an unparseable roster member without failing, which would
    leave a silent 4-of-4 (f = 0) fleet. `quorumSize`/`quorumThreshold` come
    from the quorum set the node's gate actually installed (post-clamp,
    post-fallback), so this reads the truth rather than the config. A missing
    or null field (an older binary, or a node before its first gate
    evaluation) is a refusal too.

    ``allow_unevaluated`` (a host inside its own restart window, #1608)
    accepts a node that has not evaluated its gate yet: every gate-derived
    key present and null. The config-derived keys stay strict, and a partial
    null or a missing key still refuses.
    """
    n, t, f = expected_quorum()
    expected = {
        'quorumMode': 'explicit',
        'quorumSize': n,
        'quorumThreshold': t,
        'quorumByzantineTolerance': f,
        'quorumFaultTolerant': True,
        'quorumDegenerate': False,
        'quorumAutoMembers': 0,
        'quorumGateIntersectionRefused': False,
    }
    keys = list(expected)
    if allow_unevaluated and all(k in status and status[k] is None for k in QUORUM_GATE_KEYS):
        keys = list(QUORUM_CONFIG_KEYS)
    for key in keys:
        want, got = expected[key], status.get(key)
        # `type(...) is` so True never passes as 1 or 0 as False.
        if got is None or type(got) is not type(want) or got != want:
            raise Gate('installed quorum is not explicit %d-of-%d with f=%d: %s=%r'
                       % (t, n, f, key, got))


# Degrade-step actions (discovery_plan.DEGRADE_ACTIONS, #1585) split by what
# the restart gate may relax. Only a scheduled stop/kill-then-start may change
# the node process; isolate and netem never relax anything.
RESTART_ACTIONS = frozenset({'stop_restart', 'kill_restart', 'stop_restart_pair'})
HOLD_ACTIONS = frozenset({'isolate', 'netem'})
WINDOW_KEYS = frozenset({'step', 'action', 'fault_start', 'restore_at', 'window_end'})


def check_resources(records, baseline, now, window=None):
    """Gate one host's observer ring against its stored baseline.

    ``window`` is the caller's view of the active ``degrade_steps`` entry
    (#1585) whose ``target_hosts`` names this host, in absolute seconds:
    ``{step, action, fault_start, restore_at, window_end}``. Pass ``None``
    when no active step names the host: the gate is then exactly the strict
    one and returns ``None``. ``isolate`` and ``netem`` windows relax nothing.

    For a restart action, rows dated in ``[fault_start, window_end)`` may be
    observer ``error`` rows (the unit has no live process), and at most one
    ``(pid, start)`` change from ``baseline`` is accepted, first observed in
    that interval. ``restarts``, ``binary``, ``config`` and ``node_key`` must
    never change. When the ring shows the accepted change it returns a copy
    of ``baseline`` with only ``pid`` and ``start`` replaced, for the caller
    to persist (``rss`` and the rest stay the launch values, so post-idle
    RSS growth is still measured from launch); otherwise ``None``.

    Being pure, the gate sees only the ~150 s ring. Against the returned
    baseline, a further change is a ``Gate`` while the ring still shows the
    replaced process, or when the replaced process was alive at or after
    ``restore_at`` (the restored process itself). A caller should still
    persist at most one re-baseline per step, since a second restart whose
    predecessor has left the ring entirely is indistinguishable here.
    """
    if window is None:
        _check_resources_strict(records, baseline, now)
        return None
    if set(window) != WINDOW_KEYS or not (
            window['fault_start'] <= window['restore_at'] <= window['window_end']):
        raise Gate('malformed degrade window')
    if window['action'] in HOLD_ACTIONS:
        _check_resources_strict(records, baseline, now)
        return None
    if window['action'] not in RESTART_ACTIONS:
        raise Gate('unknown degrade action')
    return _check_resources_restart(records, baseline, now, window)


def _check_resources_strict(records, baseline, now):
    """The unrelaxed gate: any restart, error row or identity change halts."""
    if not records or now-records[-1]['at'] > 60 or records[-1]['at'] > now+10:
        raise Gate('stale required observer')
    latest = records[-1]
    for row in records:
        if row.get('error'):
            raise Gate('observer cannot read required metrics')
        if not row.get('config') or not row.get('node_key'):
            raise Gate('observer configuration identity missing')
        if (row['pid'],row['start'],row['restarts'],row['binary']) != (
                baseline['pid'],baseline['start'],baseline['restarts'],baseline['binary']):
            raise Gate('unexpected node restart or binary change')
        if any(row.get(k)!=baseline.get(k) for k in ('config','node_key')):
            raise Gate('node configuration or peer identity changed')
        if row['disk_free'] < max(row['disk_total']*.20, 2*1024**3):
            raise Gate('disk headroom')
    if len(records) >= 2 and all(r['mem_available'] < max(r['mem_total']*.15,256*1024**2)
                                  for r in records[-2:]):
        raise Gate('low available host memory')
    if len(records) >= 25 and records[-1]['at']-records[-25]['at'] >= 115 and all(
            r['swap'] > 64*1024**2 for r in records[-25:]):
        raise Gate('sustained process swap')


def _check_resources_restart(records, baseline, now, window):
    if not records or now-records[-1]['at'] > 60 or records[-1]['at'] > now+10:
        raise Gate('stale required observer')
    lo, hi = window['fault_start'], window['window_end']
    live = []
    for row in records:
        if row.get('error'):
            if lo <= row['at'] < hi:
                continue  # stopped by the step: no process, no metrics
            raise Gate('observer cannot read required metrics')
        if not row.get('config') or not row.get('node_key'):
            raise Gate('observer configuration identity missing')
        # Restart=no (#1589): an automatic restart is never the executor's.
        if (row['restarts'],row['binary']) != (baseline['restarts'],baseline['binary']):
            raise Gate('unexpected node restart or binary change')
        if any(row.get(k)!=baseline.get(k) for k in ('config','node_key')):
            raise Gate('node configuration or peer identity changed')
        if row['disk_free'] < max(row['disk_total']*.20, 2*1024**3):
            raise Gate('disk headroom')
        live.append(row)
    # Distinct processes in ring order, each with the first row showing it.
    runs = []
    for row in live:
        if not runs or (row['pid'],row['start']) != runs[-1][0]:
            runs.append(((row['pid'],row['start']),row))
    keys = [key for key,_ in runs]
    base = (baseline['pid'],baseline['start'])
    if len(keys) > 2 or len(set(keys)) != len(keys):
        raise Gate('unexpected node restart or binary change')  # two changes, or a process coming back
    rebaseline = None
    if keys and keys != [base]:
        if keys[-1] != base:
            # base -> new, or new alone once base has left the ring.
            if len(keys) == 2 and keys[0] != base:
                raise Gate('unexpected node restart or binary change')
            new = keys[-1]
            rebaseline = {**baseline, 'pid': new[0], 'start': new[1]}
        # else: predecessor -> base; base is this window's re-baseline.
        first = runs[-1][1]
        if not lo <= first['at'] < hi:
            raise Gate('unexpected node restart or binary change')
        # The replaced process must not outlive the scheduled restore: one
        # alive at or after restore_at is the restored process, and
        # replacing it would be a second restart in the window.
        if len(keys) == 2 and any((r['pid'],r['start']) == keys[0]
                                  and r['at'] >= window['restore_at'] for r in live):
            raise Gate('unexpected node restart or binary change')
    if len(live) >= 2 and all(r['mem_available'] < max(r['mem_total']*.15,256*1024**2)
                              for r in live[-2:]):
        raise Gate('low available host memory')
    if len(live) >= 25 and live[-1]['at']-live[-25]['at'] >= 115 and all(
            r['swap'] > 64*1024**2 for r in live[-25:]):
        raise Gate('sustained process swap')
    return rebaseline
