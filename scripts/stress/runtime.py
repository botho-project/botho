#!/usr/bin/env python3
"""Durable primitives for the bounded testnet campaign. Python standard library only."""
import asyncio
import contextlib
import email.utils
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import time
import urllib.error
import urllib.request

HOSTS = ('seed.botho.io', 'seed2.botho.io', 'faucet.botho.io',
         'eu.seed.botho.io', 'ap.seed.botho.io')
TERMINAL = ('reconciled', 'skipped', 'rejected', 'expired', 'aborted')
PENDING = ('prepared', 'submitting', 'accepted', 'unknown', 'confirmed', 'recipient_verified')


class Gate(Exception):
    """A fail-closed admission decision, retained for explicit operator review."""


class Quota(Exception):
    pass


def atomic(path, value):
    path = Path(path)
    tmp = path.with_suffix(path.suffix + '.tmp')
    with tmp.open('w') as output:
        json.dump(value, output, sort_keys=True)
        output.write('\n')
        output.flush()
        os.fsync(output.fileno())
    os.replace(tmp, path)
    fd = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


class Journal:
    def __init__(self, path, clock=time.time):
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

    def prepare(self, identifier, artifact, phase_limit, deadline):
        """Artifact is already fsynced. Reserve every input and actual fee atomically."""
        with self.transaction():
            intent = self.intent(identifier)
            if intent['state'] != 'eligible' or self.clock() >= deadline:
                raise Gate('preparation expired or intent already consumed')
            pending = self.pending()
            if len(pending) >= min(8, phase_limit):
                raise Gate('inflight limit')
            if any(r['sender'] == intent['sender'] for r in pending if r['sender'] is not None):
                raise Gate('wallet already reserved')
            attempted = self.db.execute("SELECT COUNT(*) FROM intents WHERE prepared IS NOT NULL OR submitted IS NOT NULL").fetchone()[0]
            if attempted >= 800:
                raise Gate('chain write budget')
            fees = self.db.execute('SELECT COALESCE(SUM(fee),0) FROM intents WHERE prepared IS NOT NULL').fetchone()[0]
            if not 0 < artifact['fee'] <= 5_000_000_000 or fees + artifact['fee'] > 500_000_000_000:
                raise Gate('signed fee budget')
            if artifact['bytes'] > 262144:
                raise Gate('signed byte budget')
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
            if len(recent) >= (8 if burst else 4):
                raise Quota('submission rate budget')
            recent_bytes = sum(json.loads(r['info']).get('wire_bytes', 0) for r in recent)
            if recent_bytes + wire_bytes > 2097152:
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
    def __init__(self, journal):
        self.journal = journal
        self.locks = {h: asyncio.Semaphore(2) for h in HOSTS}
        self.opener = urllib.request.build_opener(NoRedirect())

    def reserve(self, host, method, bytes_count):
        if host not in HOSTS:
            raise Gate('unlisted RPC host')
        now = self.journal.clock()
        if now < self.journal.get('backoff:'+host, 0):
            raise Quota('endpoint Retry-After')
        count = self.journal.db.execute('SELECT COUNT(*) FROM requests WHERE host=? AND at>?',
                                        (host, now-60)).fetchone()[0]
        if count >= min(50, self.journal.get('quota:'+host, 100)//2):
            raise Quota('shared endpoint request budget')
        return self.journal.db.execute('INSERT INTO requests(at,host,method,bytes) VALUES(?,?,?,?)',
                                      (now, host, method, bytes_count)).lastrowid

    async def call(self, host, method, params=None, timeout=10, write=False):
        payload = json.dumps({'jsonrpc':'2.0', 'id':1404, 'method':method, 'params':params or {}}).encode()
        async with self.locks[host]:
            request_id = self.reserve(host, method, len(payload))
            start = time.monotonic()
            def send():
                request = urllib.request.Request('https://'+host+'/rpc', payload,
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


def check_identity(status, plan):
    if (status.get('network') != plan['network'] or status.get('gitCommit') != plan['node_commit']
            or status.get('version') != '0.6.0'):
        raise Gate('network or software identity changed')


def check_resources(records, baseline, now):
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
