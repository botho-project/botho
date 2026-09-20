"""Exact individual-owner accounting checks for the inactive funded workload."""


def amount(value):
    """Canonical nonnegative decimal strings preserve u128 values in JSON."""
    assert isinstance(value, str) and value.isascii() and value.isdecimal()
    number = int(value)
    assert str(number) == value and number < 2**128
    return number


def validate_owner_accounts(row, initial, honest_owners):
    owners = row['owners']
    assert len(owners) == honest_owners + 1
    assert all(type(owner['owner']) is int for owner in owners)
    assert [owner['owner'] for owner in owners] == list(range(honest_owners + 1))
    fields = ('fees', 'capture', 'payments_sent', 'payments_received',
              'spendable', 'locked', 'accounted')
    parsed = [{field: amount(owner[field]) for field in fields} for owner in owners]
    for owner in parsed:
        assert owner['accounted'] == owner['spendable'] + owner['locked']
        assert owner['capture'] == owner['locked']  # awards cannot fund payments
        assert (owner['accounted'] + owner['fees'] + owner['payments_sent']
                == initial + owner['payments_received'] + owner['capture'])
    assert sum(o['payments_sent'] for o in parsed) == sum(o['payments_received'] for o in parsed)
    assert sum(o['capture'] for o in parsed) == amount(row['awarded'])
    assert parsed[-1]['capture'] == amount(row['attacker_capture'])
    for role, subset in (('honest', parsed[:-1]), ('attacker', parsed[-1:])):
        for owner_field, aggregate_field in (('fees', 'fees'), ('spendable', 'spendable'),
                                             ('locked', 'locked_payouts'), ('accounted', 'accounted_value')):
            assert sum(o[owner_field] for o in subset) == amount(row[f'{role}_{aggregate_field}'])
