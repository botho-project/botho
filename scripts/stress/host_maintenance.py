"""Maintenance exclusion for disposable, fixed-lifetime Ubuntu stress hosts.

Install before packages or test services. Never apply to a shared host.
The host's independent shutdown timer remains the lifetime authority.
"""
import argparse
import json
import pathlib
import re
import subprocess
import time

UNITS = ('apt-daily.timer', 'apt-daily-upgrade.timer',
         'apt-daily.service', 'apt-daily-upgrade.service')
APT_PATH = '/etc/apt/apt.conf.d/99botho-bounded-run'
APT = ('APT::Periodic::Enable "0";\n'
       'APT::Periodic::Update-Package-Lists "0";\n'
       'APT::Periodic::Unattended-Upgrade "0";\n')
RESTART_PATH = '/etc/needrestart/conf.d/99-botho-bounded-run.conf'
RESTART = "$nrconf{restart} = 'l';\n"
MARKER = '/etc/botho-bounded-run/maintenance.json'


def bootstrap(run_id, created_at, end):
    if not re.fullmatch(r'q[0-9]{8}-[a-z][a-z0-9-]*', run_id):
        raise ValueError('isolated qualification run id required')
    if type(created_at) is not int or type(end) is not int or not 0 < end-created_at <= 83*3600:
        raise ValueError('fixed lifetime must be at most 83 hours')
    marker = json.dumps({'run_id': run_id, 'created_at': created_at, 'end': end}, sort_keys=True)
    return f'''# Exclude unattended package maintenance for this disposable run.
install -d -m755 /etc/botho-bounded-run /etc/needrestart/conf.d
cat > {APT_PATH} <<'BOTHO_APT'
{APT}BOTHO_APT
cat > {RESTART_PATH} <<'BOTHO_RESTART'
{RESTART}BOTHO_RESTART
chmod 644 {APT_PATH} {RESTART_PATH}
systemctl mask --now apt-daily.timer apt-daily-upgrade.timer
# Do not interrupt dpkg if maintenance was already active during boot.
systemctl mask apt-daily.service apt-daily-upgrade.service
maintenance_wait_end=$(( $(date +%s) + 600 ))
while true; do
  maintenance_busy=0
  for maintenance_unit in apt-daily.service apt-daily-upgrade.service; do
    maintenance_state=$(systemctl show "$maintenance_unit" -p ActiveState --value)
    case "$maintenance_state" in
      active|activating|deactivating|reloading) maintenance_busy=1 ;;
      inactive|failed) ;;
      *) exit 1 ;;
    esac
  done
  [ "$maintenance_busy" -eq 0 ] && break
  [ "$(date +%s)" -lt "$maintenance_wait_end" ] || exit 1
  sleep 2
done
cat > {MARKER} <<'BOTHO_MAINTENANCE'
{marker}
BOTHO_MAINTENANCE
chmod 644 {MARKER}
'''


def validate_units(states):
    for unit in UNITS:
        state = states.get(unit, {})
        if state.get('UnitFileState') != 'masked' or state.get('ActiveState') not in ('inactive', 'failed'):
            raise RuntimeError('Automatic maintenance is not excluded: '+unit)


def check(root=pathlib.Path('/'), now=None):
    def read(name):
        path = root / name.lstrip('/')
        info = path.stat()
        if info.st_uid != 0 or info.st_mode & 0o022:
            raise RuntimeError('Maintenance policy is not root controlled: '+name)
        return path.read_text()
    marker = json.loads(read(MARKER))
    # Reuse the input checks; the marker cannot authorize an unbounded host.
    bootstrap(marker['run_id'], marker['created_at'], marker['end'])
    now = time.time() if now is None else now
    if not marker['created_at'] <= now < marker['end']:
        raise RuntimeError('Original maintenance exclusion lifetime expired')
    if read(APT_PATH) != APT or read(RESTART_PATH) != RESTART:
        raise RuntimeError('Maintenance policy content changed')
    states = {}
    for unit in UNITS:
        raw = subprocess.check_output(['systemctl', 'show', unit, '-p', 'UnitFileState', '-p', 'ActiveState'], text=True)
        states[unit] = dict(line.split('=', 1) for line in raw.splitlines() if '=' in line)
    validate_units(states)
    # Inspect the merged configuration, not only our configuration fragment.
    raw = subprocess.check_output(['apt-config', 'dump'], text=True)
    for key in ('Enable', 'Update-Package-Lists', 'Unattended-Upgrade'):
        values = re.findall(r'^APT::Periodic::'+key+r'\s+"([^"]*)";', raw, re.M)
        if values != ['0']:
            raise RuntimeError('Effective periodic APT policy changed: '+key)
    return {'at': now, 'policy': marker, 'units': states, 'verified': True}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    print(json.dumps(check(), sort_keys=True))
