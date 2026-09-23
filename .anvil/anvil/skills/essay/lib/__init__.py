"""Essay-skill-local helpers.

These modules implement in-skill primitives that the essay commands
(`essay-draft`, `essay-review`, `essay-revise`) lean on. They live here
rather than under ``anvil/lib/`` per the framework's "skill-local first,
promote on a second consumer" policy (`CLAUDE.md` §"Working on this
repo") — `anvil:essay` is presently the only reported consumer of the
cross-version claim-ledger primitive (`claim_ledger.py`, issue #1198).

If a future skill needs the same primitive, the lift to ``anvil/lib/``
is mechanical: the row shape is deliberately kept compatible with
``anvil/lib/snippets/provenance.md`` / ``anvil/lib/provenance_anchor.py``
so promotion would not require a format migration.
"""
