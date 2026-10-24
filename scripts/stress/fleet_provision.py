"""Tracked renderers for an x-series qualification fleet; pure, stdlib-only, no AWS calls.

Everything here encodes a reviewed decision: the run id format, the tags set
at creation, the security-group ingress set, the host bootstrap with its
absolute shutdown timer, and the frozen infrastructure plan. The untracked
operator `provision.py` imports these renderers and keeps only the `aws`
calls (and the plan/launch files it persists between retries).

Tag rules:
- `project=botho` + `Purpose=qualification` is the retention scheme accepted by
  `cloud_retention.py`; `SideProject=botho` is set at creation (#1576 decision 8).
- Never emit `Project` (capital P). IAM tag keys match case-insensitively, and
  `Project=botho-qualification` is denied to the botho-deploy role.
"""
import base64
import datetime
import ipaddress
import re

import host_maintenance

ROLES = ('controller', 'n1', 'n2', 'n3', 'n4', 'n5')
PRODUCER = 'n3'
GOSSIP_PORT = 17110  # Must equal the staging renderer's gossip_port (#1589).
SSH_PORT = 22
HTTPS_PORT = 443
HOST_LIFETIME = 82 * 3600       # Absolute host deadline; under the 83h maintenance cap.
NODE_LIFETIME = 81 * 3600 + 1800
PREFUND_WINDOW = 6 * 3600
# Run ids appear in hostnames (`<role>.<run_id>.internal`, one DNS label) and
# operator-side paths; keep them short.
MAX_RUN_ID = 48
RUN_ID = re.compile(r'q(?P<date>[0-9]{8})-x(?P<n>[1-9][0-9]*)-(?P<label>[a-z][a-z0-9]*(?:-[a-z0-9]+)*)')
LABEL = re.compile(r'[a-z][a-z0-9]*(?:-[a-z0-9]+)*')
W_TOKEN = re.compile(r'(?:^|-)w[0-9]+(?:-|$)')
INSTANCE_TYPES = {'controller': ('m6i.2xlarge', 80), PRODUCER: ('m6i.large', 40)}
VALIDATOR_TYPE = ('c6i.large', 40)


def _need(condition, message):
    if not condition:
        raise ValueError(message)


def validate_run_id(value):
    """Return `value` if it is a well-formed x-series run id, else raise ValueError."""
    _need(isinstance(value, str), 'run id must be a string')
    match = RUN_ID.fullmatch(value)
    _need(match is not None, 'run id must be qYYYYMMDD-xN-<label> in lowercase: ' + repr(value))
    try:
        datetime.datetime.strptime(match['date'], '%Y%m%d')
    except ValueError:
        raise ValueError('run id date is not a calendar date: ' + match['date']) from None
    _need(not W_TOKEN.search(value), 'w-series token in run id: ' + value)
    _need(len(value) <= MAX_RUN_ID, 'run id longer than %d characters' % MAX_RUN_ID)
    _need(re.fullmatch(r'q[0-9]{8}-[a-z][a-z0-9-]*', value), 'run id rejected by host_maintenance')
    return value


def run_id(date, n, label):
    """Build `qYYYYMMDD-xN-<label>`; `date` is a datetime.date or a YYYYMMDD string."""
    if isinstance(date, datetime.date):
        date = date.strftime('%Y%m%d')
    _need(isinstance(date, str) and re.fullmatch(r'[0-9]{8}', date), 'date must be YYYYMMDD')
    _need(type(n) is int and n >= 1, 'x-series number must be a positive integer')
    _need(isinstance(label, str) and LABEL.fullmatch(label), 'label must be lowercase [a-z][a-z0-9-]*: ' + repr(label))
    return validate_run_id('q%s-x%d-%s' % (date, n, label))


def _role(role):
    _need(role in ROLES, 'unknown role: ' + repr(role))
    return role


def _guard(pairs):
    """Refuse any key that collides case-insensitively, including `Project`."""
    seen = set()
    for pair in pairs:
        key = pair['Key']
        _need(key.lower() != 'project' or key == 'project', 'refusing tag key %r: IAM matches tag keys case-insensitively' % key)
        _need(key.lower() not in seen, 'duplicate tag key (case-insensitive): ' + key)
        seen.add(key.lower())
        _need(isinstance(pair['Value'], str) and pair['Value'], 'empty tag value: ' + key)
    return pairs


def _pairs(values):
    return _guard([{'Key': k, 'Value': v} for k, v in values])


def _common(run):
    return [('project', 'botho'), ('Purpose', 'qualification'), ('SideProject', 'botho'), ('Run', run)]


def tags(run, role, host_end):
    """Tags for one role's instance and its root volume (identical for both)."""
    validate_run_id(run)
    _role(role)
    _need(type(host_end) is int and host_end > 0, 'host_end must be a positive integer epoch')
    return _pairs([('Name', 'botho-%s-%s' % (run, role))] + _common(run)
                  + [('Role', role), ('ExpiresAt', str(host_end))])


def tag_specifications(run, role, host_end):
    """RunInstances `TagSpecifications`: tags are applied at creation, never afterwards."""
    return [{'ResourceType': kind, 'Tags': tags(run, role, host_end)} for kind in ('instance', 'volume')]


def security_group_name(run):
    return 'botho-' + validate_run_id(run)


def security_group_tags(run):
    validate_run_id(run)
    return _pairs(_common(run))


def security_group_specifications(run):
    return [{'ResourceType': 'security-group', 'Tags': security_group_tags(run)}]


def operator_cidrs(values):
    """Validate operator SSH sources: non-empty, distinct IPv4 /32 hosts only."""
    _need(isinstance(values, (list, tuple)) and values, 'at least one operator CIDR is required')
    out = []
    for value in values:
        _need(isinstance(value, str) and '/' in value, 'operator CIDR must be written with an explicit prefix: ' + repr(value))
        try:
            net = ipaddress.ip_network(value, strict=True)
        except ValueError:
            raise ValueError('invalid operator CIDR: ' + repr(value)) from None
        _need(net.prefixlen != 0, 'refusing world-open CIDR: ' + value)
        _need(net.version == 4 and net.prefixlen == 32, 'operator CIDR must be an IPv4 /32: ' + value)
        _need(not net.is_unspecified, 'refusing unspecified address: ' + value)
        _need(value not in out, 'duplicate operator CIDR: ' + value)
        out.append(value)
    return out


def security_group_rules(group_id, cidrs):
    """Ingress for AuthorizeSecurityGroupIngress: operator SSH plus intra-fleet traffic only."""
    _need(isinstance(group_id, str) and re.fullmatch(r'sg-[0-9a-f]{8,17}', group_id), 'invalid security group id')
    cidrs = operator_cidrs(cidrs)
    rules = [{'IpProtocol': 'tcp', 'FromPort': SSH_PORT, 'ToPort': SSH_PORT,
              'IpRanges': [{'CidrIp': cidr} for cidr in cidrs]}]
    for port in (SSH_PORT, HTTPS_PORT, GOSSIP_PORT):
        rules.append({'IpProtocol': 'tcp', 'FromPort': port, 'ToPort': port, 'UserIdGroupPairs': [{'GroupId': group_id}]})
    rules.append({'IpProtocol': 'udp', 'FromPort': GOSSIP_PORT, 'ToPort': GOSSIP_PORT, 'UserIdGroupPairs': [{'GroupId': group_id}]})
    return rules


def _lifetime(created_at, host_end):
    _need(type(created_at) is int and type(host_end) is int and created_at > 0, 'times must be integer epochs')
    _need(0 < host_end - created_at <= HOST_LIFETIME, 'host lifetime must be positive and at most 82 hours')


def on_calendar(host_end):
    """systemd OnCalendar value for the absolute stop timer."""
    return datetime.datetime.fromtimestamp(host_end, datetime.timezone.utc).strftime('%Y-%m-%d %H:%M:%S UTC')


def bootstrap(role, run, created_at, host_end):
    """EC2 user data: absolute-stop timer, maintenance exclusion, packages, role/run markers."""
    _role(role)
    validate_run_id(run)
    _lifetime(created_at, host_end)
    maintenance = host_maintenance.bootstrap(run, created_at, host_end)
    return f'''#!/bin/bash
set -euo pipefail
umask 077
mkdir -p /etc/botho-qualification
printf '%s\\n' '{host_end}' > /etc/botho-qualification/absolute-stop
cat > /etc/systemd/system/botho-qualification-stop.service <<'EOF'
[Unit]
Description=Absolute qualification host shutdown
[Service]
Type=oneshot
ExecStart=/usr/bin/systemctl poweroff
EOF
cat > /etc/systemd/system/botho-qualification-stop.timer <<'EOF'
[Unit]
Description=Immutable qualification lifetime
[Timer]
OnCalendar={on_calendar(host_end)}
Persistent=true
Unit=botho-qualification-stop.service
[Install]
WantedBy=timers.target
EOF
chmod 644 /etc/systemd/system/botho-qualification-stop.*
systemctl daemon-reload
systemctl enable --now botho-qualification-stop.timer
if [ "$(date +%s)" -ge {host_end} ]; then systemctl poweroff; exit 1; fi
{maintenance}
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get -o DPkg::Lock::Timeout=600 upgrade -y -qq
apt-get -o DPkg::Lock::Timeout=600 install -y -qq nginx python3 python3-lmdb openssl rsync git
useradd --system --create-home --shell /bin/bash botho
useradd --system --create-home --shell /bin/bash bothoctl
useradd --system --create-home --shell /bin/bash botho-pin
install -d -m 700 /etc/ssh/authorized_keys
cat > /etc/ssh/sshd_config.d/40-botho-qualification.conf <<'EOF'
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
AuthorizedKeysFile .ssh/authorized_keys /etc/ssh/authorized_keys/%u
EOF
install -d -m755 /run/sshd
/usr/sbin/sshd -t
systemctl reload-or-restart ssh
install -d -m 700 -o ubuntu -g ubuntu /home/ubuntu/qualification
printf '%s\\n' '{role}' > /etc/botho-qualification/role
printf '%s\\n' '{run}' > /etc/botho-qualification/run-id
date --iso-8601=seconds > /etc/botho-qualification/bootstrap-complete
'''


def infrastructure_plan(run, created_at, *, region, subnet, vpc, image, cidrs, scope, timing_mode='normal'):
    """The plan the operator freezes to `infrastructure-plan.json` before any launch."""
    validate_run_id(run)
    _need(type(created_at) is int and created_at > 0, 'created_at must be an integer epoch')
    _need(isinstance(region, str) and re.fullmatch(r'[a-z]{2}(?:-[a-z]+)+-[0-9]', region), 'invalid region')
    _need(isinstance(subnet, str) and re.fullmatch(r'subnet-[0-9a-f]{8,17}', subnet), 'invalid subnet id')
    _need(isinstance(vpc, str) and re.fullmatch(r'vpc-[0-9a-f]{8,17}', vpc), 'invalid vpc id')
    _need(isinstance(image, str) and re.fullmatch(r'ami-[0-9a-f]{8,17}', image), 'invalid image id')
    _need(isinstance(scope, str) and scope.strip(), 'scope description required')
    _need(timing_mode == 'normal', 'only normal timing is supported')
    host_end = created_at + HOST_LIFETIME
    _lifetime(created_at, host_end)
    roles = []
    for role in ROLES:
        kind, disk = INSTANCE_TYPES.get(role, VALIDATOR_TYPE)
        roles.append({'role': role, 'type': kind, 'disk_gib': disk})
    return {'timing_mode': timing_mode, 'run_id': run, 'created_at': created_at,
            'host_end': host_end, 'node_end': created_at + NODE_LIFETIME,
            'prefund_end': created_at + PREFUND_WINDOW, 'region': region, 'subnet': subnet,
            'vpc': vpc, 'image': image, 'operator_cidrs': operator_cidrs(cidrs),
            'roles': roles, 'scope': scope}


def launch_request(plan, role, group_id, key_name='botho-nodes'):
    """RunInstances `--cli-input-json` body for one role, with tags set at creation."""
    run = validate_run_id(plan['run_id'])
    _need(plan['host_end'] == plan['created_at'] + HOST_LIFETIME, 'plan host_end is not created_at + 82h')
    _need(isinstance(group_id, str) and re.fullmatch(r'sg-[0-9a-f]{8,17}', group_id), 'invalid security group id')
    item = [r for r in plan['roles'] if r['role'] == _role(role)]
    _need(len(item) == 1, 'role missing from plan: ' + role)
    item = item[0]
    script = bootstrap(role, run, plan['created_at'], plan['host_end'])
    return {'ImageId': plan['image'], 'InstanceType': item['type'], 'MinCount': 1, 'MaxCount': 1,
            'KeyName': key_name, 'ClientToken': 'botho-%s-%s' % (run, role),
            'InstanceInitiatedShutdownBehavior': 'stop',
            'MetadataOptions': {'HttpTokens': 'required', 'HttpEndpoint': 'enabled', 'HttpPutResponseHopLimit': 1},
            'BlockDeviceMappings': [{'DeviceName': '/dev/sda1', 'Ebs': {'VolumeSize': item['disk_gib'], 'VolumeType': 'gp3',
                                                                         'Encrypted': True, 'DeleteOnTermination': False}}],
            'NetworkInterfaces': [{'DeviceIndex': 0, 'SubnetId': plan['subnet'], 'AssociatePublicIpAddress': True, 'Groups': [group_id]}],
            'TagSpecifications': tag_specifications(run, role, plan['host_end']),
            'UserData': base64.b64encode(script.encode()).decode()}
