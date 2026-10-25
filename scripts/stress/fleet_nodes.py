#!/usr/bin/env python3
"""Tracked validator staging renderer for x-series fleets; pure, stdlib-only, no SSH.

Replaces the config-rendering half of the untracked w-series `stage-nodes.py`
(#1589, part of #1576). For each validator it renders `config.toml`, the
systemd node unit with its node-stop service/timer, and the nginx site. It also
renders the `targets` and `observers[].node` fragment for `staging.py`'s
deployment profile. The operator's untracked installer (rsync plus
`install.sh`) consumes the payload directories. Nothing here opens a network
connection.

Reviewed decisions encoded here:
- 4-of-5 explicit quorum (`QUORUM_THRESHOLD`): `explicit_quorum_size()` counts
  self, so 4 members plus self with threshold 4 tolerates f = 1 (#1546, #1576).
- `fallback_seeds = false` and DNS seeds off, so a validator with an empty or
  unreachable peer list never falls back to the live public seeds (#1578).
- `Restart=no`: any restart not made by the fault executor is a failure
  (#1586/#1587).
- nginx `= /metrics` goes to the standalone metrics server on `metrics_port`.
  The RPC listener's `/metrics` lacks the loop histogram that #1591 reads.
- No `ledger_rules` key: main's config has no such field (#1571).
- Ports come from `fleet_provision`, so the security group and the configs
  cannot drift apart.

Paths, unit names and hostnames are derived from `run_id`, so a new run never
needs a token-rename step.
"""
import argparse
import errno
import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import sys
import time
import tomllib

import fleet_provision as fp
from observer import node_config
from runtime import HOSTS, QUORUM_THRESHOLD, campaign_targets

VALIDATORS = tuple(role for role in fp.ROLES if role != 'controller')
assert len(VALIDATORS) == len(HOSTS) == 5
# Explicit-mode threshold for five validators: 4-of-5, f = 1 (#1576). A module
# constant on purpose; it is not a profile knob. Imported from runtime so the
# controller's check_quorum (#1605) verifies the same value this renders.
assert QUORUM_THRESHOLD == 4
MIN_PEERS = 4
MINT_THREADS = 1
NODE_USER = 'botho'  # Created by fleet_provision.bootstrap(); observers share it.
PROFILE_KEYS = {'run_id', 'producer_role', 'node_end', 'rpc_requests_per_minute'}
STAGING_RUN_ID = re.compile(r'[a-z][a-z0-9-]{7,63}')  # staging.prepare()'s run_id rule.
PRIVATE_NETS = tuple(ipaddress.ip_network(n) for n in ('10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16'))
PEER_ID = re.compile(r'12D3KooW[1-9A-HJ-NP-Za-km-z]{44}')
BASE58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
# Identity multihash (0x00, length 0x24) of a protobuf PublicKey {KeyType
# Ed25519 = 1 (08 01), Data: 32 bytes (12 20)}. Node keys are always Ed25519
# (`Keypair::generate_ed25519()` in botho/src/network/node_key.rs).
ED25519_PEER_PREFIX = bytes.fromhex('002408011220')
# Mirrors MAX_TESTNET_MINT_WINDOW_SECS in botho/src/commands/run.rs, which
# `TestnetMintWindow::new` enforces at node start: a node_end further out than
# this makes the producer refuse `--testnet-mint-until`.
MAX_TESTNET_MINT_WINDOW_SECS = 82 * 60 * 60
MNEMONIC = re.compile(r'[a-z]+(?: [a-z]+)*')
MULTIADDR = re.compile(r'/ip4/(?P<ip>[0-9.]+)/tcp/(?P<port>[0-9]+)/p2p/(?P<peer>[1-9A-HJ-NP-Za-km-z]+)')
FORBIDDEN = ('ledger_rules', 'botho.io') + HOSTS
PAYLOAD_FILES = ('config.toml', 'node.service', 'node-stop.service', 'node-stop.timer', 'nginx.conf', 'paths.json')

# The exact key set at each table level. A typo is silently ignored by the node
# (no deny_unknown_fields), so the renderer refuses anything else.
KEYS = {
    (): {'network_type', 'network', 'minting', 'faucet', 'telemetry', 'rpc'},
    ('wallet',): {'mnemonic'},
    ('network',): {'gossip_port', 'rpc_port', 'metrics_port', 'node_key_path', 'bootstrap_peers',
                   'fallback_seeds', 'dns_seeds', 'quorum'},
    ('network', 'dns_seeds'): {'enabled'},
    ('network', 'quorum'): {'mode', 'threshold', 'min_peers', 'max_auto_members', 'members'},
    ('minting',): {'enabled', 'threads'},
    ('faucet',): {'enabled'},
    ('telemetry',): {'enabled'},
    ('rpc',): {'requests_per_minute'},
}


def _need(condition, message):
    if not condition:
        raise ValueError(message)


def ports():
    """(gossip, rpc, metrics) from fleet_provision; RPC and metrics must differ."""
    gossip, rpc, metrics = fp.GOSSIP_PORT, fp.RPC_PORT, fp.METRICS_PORT
    for port in (gossip, rpc, metrics):
        _need(type(port) is int and 1024 <= port <= 65535, 'invalid port: %r' % (port,))
    _need(rpc != metrics, 'rpc_port and metrics_port must differ: the RPC /metrics lacks the loop histogram')
    _need(gossip not in (rpc, metrics), 'gossip_port collides with an HTTP listener')
    return gossip, rpc, metrics


def validate_profile(profile):
    _need(isinstance(profile, dict) and set(profile) == PROFILE_KEYS,
          'profile must have exactly: ' + ', '.join(sorted(PROFILE_KEYS)))
    run = profile['run_id']
    try:
        fp.validate_run_id(run)
    except ValueError as error:
        raise ValueError('malformed run_id: %s' % error) from None
    _need(STAGING_RUN_ID.fullmatch(run), 'run_id rejected by staging.py')
    _need(profile['producer_role'] in VALIDATORS, 'producer_role must be one of n1..n5')
    _need(type(profile['node_end']) is int and profile['node_end'] > 0, 'node_end must be a positive integer epoch')
    rpm = profile['rpc_requests_per_minute']
    _need(type(rpm) is int and 1 <= rpm <= 10000, 'rpc_requests_per_minute must be an integer in 1..10000')
    return profile


def _now():
    return int(time.time())


def check_node_end(profile, now=None):
    """Require `now < node_end <= now + MAX_TESTNET_MINT_WINDOW_SECS`.

    The producer's `--testnet-mint-until node_end` is refused at node start
    beyond the Rust bound, and a past node_end would stop every node at once.
    `now` is injectable for tests; `None` reads the wall clock.
    """
    now = _now() if now is None else now
    end = validate_profile(profile)['node_end']
    _need(end > now, 'node_end is not in the future')
    _need(end <= now + MAX_TESTNET_MINT_WINDOW_SECS,
          'node_end is more than %d h ahead (MAX_TESTNET_MINT_WINDOW_SECS)' % (MAX_TESTNET_MINT_WINDOW_SECS // 3600))
    return end


def paths(run):
    """Every on-host name for one run, derived from the run id alone."""
    fp.validate_run_id(run)
    package, data = '/opt/botho-' + run, '/var/lib/botho-' + run
    return {'package': package, 'binary_path': package + '/bin/botho', 'data_dir': data,
            'config_path': data + '/config.toml', 'node_key_path': data + '/node_key',
            'tls_dir': '/etc/botho-' + run, 'node_unit': 'botho-%s.service' % run,
            'stop_service': 'botho-%s-node-stop.service' % run,
            'stop_timer': 'botho-%s-node-stop.timer' % run, 'nginx_site': 'botho-' + run}


def hostname(role, run):
    return '%s.%s.internal' % (role, run)


def _private_ip(value, role):
    try:
        address = ipaddress.ip_address(value)
    except ValueError:
        raise ValueError('invalid private_ip for %s: %r' % (role, value)) from None
    _need(address.version == 4 and any(address in net for net in PRIVATE_NETS),
          'inventory address for %s is not RFC1918: %s' % (role, value))
    return str(address)


def validate_inventory(inventory, run):
    """Return {role: private_ip} for all six roles; every address RFC1918 and distinct."""
    _need(isinstance(inventory, list), 'inventory must be a list of role records')
    out = {}
    for item in inventory:
        _need(isinstance(item, dict) and isinstance(item.get('role'), str), 'inventory records need a role')
        role = item['role']
        _need(role in fp.ROLES, 'unknown inventory role: %r' % role)
        _need(role not in out, 'duplicate inventory role: ' + role)
        if 'hostname' in item:
            _need(item['hostname'] == hostname(role, run), 'inventory hostname for %s is not derived from run_id' % role)
        out[role] = _private_ip(item.get('private_ip'), role)
    _need(set(out) == set(fp.ROLES), 'inventory must list exactly: ' + ', '.join(fp.ROLES))
    _need(len(set(out.values())) == len(out), 'inventory private IPs must be distinct')
    return {role: out[role] for role in fp.ROLES}


def b58encode(data):
    number = int.from_bytes(data, 'big')
    out = ''
    while number:
        number, digit = divmod(number, 58)
        out = BASE58[digit] + out
    return '1' * (len(data) - len(data.lstrip(b'\0'))) + out


def b58decode(text):
    number = 0
    for char in text:
        index = BASE58.find(char)
        _need(index >= 0, 'invalid base58 character')
        number = number * 58 + index
    body = number.to_bytes((number.bit_length() + 7) // 8, 'big')
    return b'\0' * (len(text) - len(text.lstrip('1'))) + body


def parse_peer_id(value):
    """Return the 32-byte Ed25519 public key behind a base58 PeerId, or raise.

    Stricter than libp2p: only the identity-multihash Ed25519 form
    `00 24 08 01 12 20 || key` is accepted (the fleet has no RSA keys), and the
    string must re-encode to itself, so a regex-valid but undecodable ID that
    the node would silently drop from its quorum (#1605) is refused here.
    """
    _need(isinstance(value, str) and PEER_ID.fullmatch(value), 'peer_id is not base58 12D3KooW... form')
    raw = b58decode(value)
    _need(len(raw) == len(ED25519_PEER_PREFIX) + 32, 'peer_id is not a 38-byte Ed25519 identity multihash')
    _need(raw.startswith(ED25519_PEER_PREFIX), 'peer_id multihash prefix is not Ed25519 identity')
    _need(b58encode(raw) == value, 'peer_id does not round-trip through base58')
    return raw[len(ED25519_PEER_PREFIX):]


def validate_identities(identities):
    """Return {role: peer_id} for n1..n5; distinct, so no member can equal self."""
    _need(isinstance(identities, dict) and set(identities) >= set(VALIDATORS), 'identities required for n1..n5')
    out = {}
    for role in VALIDATORS:
        record = identities[role]
        peer = record.get('peer_id') if isinstance(record, dict) else None
        try:
            parse_peer_id(peer)
        except ValueError as error:
            raise ValueError('invalid peer_id for %s: %s' % (role, error)) from None
        _need(peer not in out.values(), 'duplicate peer_id for %s: self would appear in members' % role)
        out[role] = peer
    return out


def validate_mnemonic(value):
    _need(isinstance(value, str) and MNEMONIC.fullmatch(value) and len(value.split()) in (12, 15, 18, 21, 24),
          'producer mnemonic must be 12-24 lowercase words')
    return value


def read_private(path):
    """Read an owner-only regular file (no symlink); used for the producer mnemonic.

    Opened with O_NOFOLLOW, and every check runs on the opened fd, so there is
    no lstat-then-open window in which the path could be swapped (#1605).
    """
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError as error:
        # ELOOP (Linux, macOS) or EMLINK (FreeBSD): the final component is a symlink.
        if error.errno in (errno.ELOOP, errno.EMLINK):
            raise ValueError('producer mnemonic must be a regular file, not a symlink') from None
        raise ValueError('cannot open producer mnemonic: %s' % error.strerror) from None
    try:
        info = os.fstat(fd)
        _need(stat.S_ISREG(info.st_mode), 'producer mnemonic must be a regular file, not a symlink')
        _need(info.st_uid == os.geteuid(), 'producer mnemonic file must be owned by the current user')
        _need(info.st_mode & 0o077 == 0, 'producer mnemonic file must be owner-only (mode 0600)')
    except BaseException:
        os.close(fd)
        raise
    with os.fdopen(fd, 'r') as stream:
        return stream.read().strip()


def _role(role):
    _need(role in VALIDATORS, 'not a validator role: %r' % (role,))
    return role


def _toml(value):
    if isinstance(value, bool):
        return 'true' if value else 'false'
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        _need(all(32 <= ord(c) < 127 for c in value), 'non-printable value in config')
        return json.dumps(value)
    return '[\n' + ''.join('    %s,\n' % _toml(item) for item in value) + ']'


def _table(name, rows):
    head = '[%s]\n' % name if name else ''
    return head + ''.join('%s = %s\n' % (key, _toml(value)) for key, value in rows) + '\n'


def render_config(role, inventory, identities, profile, producer_mnemonic=None):
    """`config.toml` for one validator; refuses anything outside the reviewed shape."""
    _role(role)
    profile = validate_profile(profile)
    run = profile['run_id']
    ips = validate_inventory(inventory, run)
    peers = validate_identities(identities)
    gossip, rpc, metrics = ports()
    producer = role == profile['producer_role']
    if producer:
        _need(producer_mnemonic is not None, 'producer role requires its wallet mnemonic')
        validate_mnemonic(producer_mnemonic)
    else:
        _need(producer_mnemonic is None, 'only the producer role carries a wallet mnemonic')
    others = [r for r in VALIDATORS if r != role]
    text = _table(None, [('network_type', 'testnet')])
    if producer:
        text += _table('wallet', [('mnemonic', producer_mnemonic)])
    text += _table('network', [
        ('gossip_port', gossip), ('rpc_port', rpc), ('metrics_port', metrics),
        ('node_key_path', paths(run)['node_key_path']),
        ('bootstrap_peers', ['/ip4/%s/tcp/%d/p2p/%s' % (ips[r], gossip, peers[r]) for r in others]),
        ('fallback_seeds', False)])
    text += _table('network.dns_seeds', [('enabled', False)])
    text += _table('network.quorum', [
        ('mode', 'explicit'), ('threshold', QUORUM_THRESHOLD), ('min_peers', MIN_PEERS),
        ('max_auto_members', 0), ('members', [peers[r] for r in others])])
    text += _table('minting', [('enabled', producer), ('threads', MINT_THREADS)])
    text += _table('faucet', [('enabled', False)])
    text += _table('telemetry', [('enabled', False)])
    text += _table('rpc', [('requests_per_minute', profile['rpc_requests_per_minute'])])
    check_config(role, text, inventory, identities, profile)
    return text


def _keys(doc, path=()):
    """Yield (table path, key set) for every table in a parsed config."""
    yield path, set(doc)
    for key, value in doc.items():
        if isinstance(value, dict):
            yield from _keys(value, path + (key,))


def byzantine_tolerance(config, self_peer):
    """Mirror QuorumConfig::explicit_byzantine_tolerance(): f = min(n - t, 2t - n - 1)."""
    quorum = config['network']['quorum']
    n = len({m for m in quorum['members'] if m != self_peer}) + 1
    t = quorum['threshold']
    return max(0, min(n - t, 2 * t - n - 1))


def check_config(role, text, inventory, identities, profile):
    """Parse a rendered config and refuse any deviation from the reviewed shape."""
    _role(role)
    run = validate_profile(profile)['run_id']
    ips = validate_inventory(inventory, run)
    peers = validate_identities(identities)
    gossip, rpc, metrics = ports()
    producer = role == profile['producer_role']
    for word in FORBIDDEN:
        _need(word not in text, 'rendered config contains forbidden text: ' + word)
    doc = tomllib.loads(text)
    expected = dict(KEYS)
    if producer:
        expected[()] = expected[()] | {'wallet'}
    else:
        del expected[('wallet',)]
    seen = dict(_keys(doc))
    _need(set(seen) == set(expected), 'unexpected config tables: %r' % sorted(set(seen) ^ set(expected)))
    for path, keys in seen.items():
        _need(keys == expected[path], 'config table [%s] has keys %r' % ('.'.join(path), sorted(keys ^ expected[path])))
    net, quorum = doc['network'], doc['network']['quorum']
    _need(doc['network_type'] == 'testnet', 'network_type must be testnet')
    _need((net['gossip_port'], net['rpc_port'], net['metrics_port']) == (gossip, rpc, metrics), 'ports differ from fleet_provision')
    _need(net['node_key_path'] == paths(run)['node_key_path'], 'node_key_path not derived from run_id')
    _need(net['fallback_seeds'] is False and net['dns_seeds']['enabled'] is False, 'public seed fallback must be off')
    _need(quorum['mode'] == 'explicit' and quorum['threshold'] == QUORUM_THRESHOLD, 'quorum must be explicit 4-of-5')
    _need(quorum['max_auto_members'] == 0, 'auto quorum members must be off')
    _need(peers[role] not in quorum['members'], 'self in quorum members: ' + role)
    others = {peers[r] for r in VALIDATORS if r != role}
    _need(len(quorum['members']) == len(others) and set(quorum['members']) == others,
          'quorum members must be exactly the four other validators')
    _need(byzantine_tolerance(doc, peers[role]) == 1, 'quorum does not tolerate exactly one fault')
    allowed = {ips[r] for r in VALIDATORS if r != role}
    seen_peers = set()
    for addr in net['bootstrap_peers']:
        match = MULTIADDR.fullmatch(addr)
        _need(match is not None, 'malformed bootstrap multiaddr: ' + addr)
        _private_ip(match['ip'], role)
        _need(match['ip'] in allowed and int(match['port']) == gossip, 'bootstrap peer is not a fleet validator on the gossip port: ' + addr)
        _need(match['peer'] in others, 'bootstrap peer id is not a fleet validator: ' + addr)
        seen_peers.add(match['peer'])
    _need(seen_peers == others and len(net['bootstrap_peers']) == 4, 'bootstrap peers must be the four other validators')
    _need(doc['minting'] == {'enabled': producer, 'threads': MINT_THREADS}, 'only the producer mints')
    _need(doc['faucet']['enabled'] is False and doc['telemetry']['enabled'] is False, 'faucet and telemetry must be off')
    if producer:
        validate_mnemonic(doc['wallet']['mnemonic'])
    return doc


def render_node_unit(role, inventory, profile, now=None):
    """The validator's systemd unit; the producer alone carries the mint flags."""
    _role(role)
    profile = validate_profile(profile)
    check_node_end(profile, now)
    run = profile['run_id']
    ips = validate_inventory(inventory, run)
    p = paths(run)
    producer = role == profile['producer_role']
    mint = ' --mint --mint-threads %d --testnet-mint-until %d' % (MINT_THREADS, profile['node_end']) if producer else ''
    allow = ''.join('IPAddressAllow=%s/32\n' % ips[r] for r in fp.ROLES)
    text = f'''[Unit]
Description=Isolated x-series qualification validator {role} ({run})
After=network-online.target
Wants=network-online.target
[Service]
User={NODE_USER}
Group={NODE_USER}
UMask=0077
Environment=RUST_LOG=warn,consensus_diagnostics=debug,sync_diagnostics=debug,ledger_diagnostics=info
LogRateLimitIntervalSec=0
ExecStart={p['binary_path']} --testnet --config {p['config_path']} run{mint}
Restart=no
MemoryMax={'5G' if producer else '2G'}
MemorySwapMax=0
CPUQuota=200%
TasksMax=512
RuntimeMaxSec={fp.NODE_LIFETIME}
TimeoutStopSec=30
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths={p['data_dir']}
PrivateTmp=yes
IPAddressDeny=any
IPAddressAllow=localhost
{allow}[Install]
WantedBy=multi-user.target
'''
    for word in FORBIDDEN:
        _need(word not in text, 'rendered unit contains forbidden text: ' + word)
    return text


def render_node_stop(profile, now=None):
    """(service, timer) that stop the node unit at the absolute `node_end`."""
    profile = validate_profile(profile)
    check_node_end(profile, now)
    p = paths(profile['run_id'])
    service = f'''[Unit]
Description=Stop the qualification validator at node_end
[Service]
Type=oneshot
ExecStart=/usr/bin/systemctl stop {p['node_unit']}
'''
    timer = f'''[Unit]
Description=Absolute qualification validator stop
[Timer]
OnCalendar={fp.on_calendar(profile['node_end'])}
Persistent=true
Unit={p['stop_service']}
[Install]
WantedBy=timers.target
'''
    return service, timer


def render_nginx(role, inventory, profile):
    """TLS site exposing only `= /rpc` and `= /metrics`; everything else is 404."""
    _role(role)
    profile = validate_profile(profile)
    run = profile['run_id']
    validate_inventory(inventory, run)
    _, rpc, metrics = ports()
    p = paths(run)
    return f'''server {{
    listen 443 ssl;
    server_name {hostname(role, run)};
    ssl_certificate {p['tls_dir']}/tls.crt;
    ssl_certificate_key {p['tls_dir']}/tls.key;
    client_max_body_size 1m;
    location = /rpc {{ proxy_pass http://127.0.0.1:{rpc}/rpc; proxy_read_timeout 30s; }}
    location = /metrics {{ proxy_pass http://127.0.0.1:{metrics}/metrics; proxy_read_timeout 30s; }}
    location / {{ return 404; }}
}}
'''


def render_staging_fragment(inventory, profile):
    """`targets` plus `observers[].node` for staging.py's profile, in HOSTS order."""
    profile = validate_profile(profile)
    run = profile['run_id']
    validate_inventory(inventory, run)
    p = paths(run)
    node = {key: p[key] for key in ('node_unit', 'binary_path', 'data_dir', 'config_path', 'node_key_path')}
    targets, observers = [], []
    for logical, role in zip(HOSTS, VALIDATORS):
        host = hostname(role, run)
        targets.append({'role': logical, 'rpc_url': 'https://%s/rpc' % host,
                        'observer_ssh_target': '%s@%s' % (NODE_USER, host)})
        observers.append({'role': logical, 'node': node_config(dict(node))})
    campaign_targets({'targets': targets}, {'endpoints': [t['rpc_url'] for t in targets]})
    return {'targets': targets, 'observers': observers}


def render_payloads(inventory, identities, profile, producer_mnemonic, now=None):
    """{role: {file name: text}} plus the staging fragment; no I/O."""
    profile = validate_profile(profile)
    now = _now() if now is None else now
    service, timer = render_node_stop(profile, now)
    names = json.dumps(paths(profile['run_id']), indent=2, sort_keys=True) + '\n'
    out = {}
    for role in VALIDATORS:
        mnemonic = producer_mnemonic if role == profile['producer_role'] else None
        out[role] = {'config.toml': render_config(role, inventory, identities, profile, mnemonic),
                     'node.service': render_node_unit(role, inventory, profile, now),
                     'node-stop.service': service, 'node-stop.timer': timer,
                     'nginx.conf': render_nginx(role, inventory, profile), 'paths.json': names}
    return out, render_staging_fragment(inventory, profile)


def _write(path, text):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as stream:
        stream.write(text)
    os.chmod(path, 0o600)


def _load(path):
    return json.loads(Path(path).read_text())


def render_command(args):
    profile, inventory = _load(args.profile), _load(args.inventory)
    identities = Path(args.identities)
    identities = _load(identities / 'nodes.json' if identities.is_dir() else identities)
    mnemonic = read_private(args.producer_mnemonic)
    payloads, fragment = render_payloads(inventory, identities, profile, mnemonic)
    output = Path(args.output)
    _need(not output.exists() and not output.is_symlink(), 'output must not exist: ' + str(output))
    output.mkdir(mode=0o700, parents=False)
    for role, files in payloads.items():
        directory = output / ('payload-' + role)
        directory.mkdir(mode=0o700)
        for name, text in files.items():
            _write(directory / name, text)
    _write(output / 'staging-fragment.json', json.dumps(fragment, indent=2) + '\n')
    return sorted(payloads)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest='command', required=True)
    render = sub.add_parser('render', help='write one payload directory per validator; no SSH')
    render.add_argument('--profile', required=True)
    render.add_argument('--inventory', required=True)
    render.add_argument('--identities', required=True, help='nodes.json, or a directory containing it')
    render.add_argument('--producer-mnemonic', required=True, help='owner-only file')
    render.add_argument('--output', required=True, help='fresh directory')
    args = parser.parse_args(argv)
    try:
        roles = render_command(args)
    except (ValueError, OSError) as error:
        print('refused: %s' % error, file=sys.stderr)
        return 2
    print(json.dumps({'output': args.output, 'roles': roles}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
