"""Skill-local lib for `anvil:ip-search` (issue #957).

Modules (composition order):

- ``brief_features``: parse a thread's ``BRIEF.md`` into the inventive-feature
  inventory the search is derived from (``§3 — Inventive features`` by
  preference, with documented fallbacks). Pure function of the brief text;
  no network, no writes.
- ``query``: turn features (or an operator-supplied ``--query`` string) into
  deterministic per-feature search queries plus the Google-Patents URL used
  by the no-key fallback path.
- ``corpus``: stdlib-only (``urllib.request``) clients for the live patent
  corpora — PatentsView Search (primary) and the USPTO Open Data Portal
  (secondary) — with API-key resolution from the environment, exponential
  backoff, and a hard rule that **a missing / rejected key or an
  unparseable response degrades, never crashes**.
- ``reference``: render one search hit into the exact
  ``<thread>/prior-art/<slug>.md`` shape that ``ip-uspto-prior-art`` /
  ``ip-uspto-provisional-prior-art`` already parse, and own the write-scope
  guard that refuses any destination outside ``<thread>/prior-art/``.
- ``orchestrate``: single ``run()`` entry composing brief → queries →
  corpus → references → writes, and building the operator-facing report.

Posture (mirrors the rest of the ip suite): this is a **drafting aid**, not
a professional or attorney clearance search. Every emitted reference file
carries that disclaimer in its body.

Write scope: the ONLY directory this skill ever writes is
``<thread>/prior-art/``. Version dirs (``<thread>.{N}/``) and critic
siblings (``<thread>.{N}.<tag>/``) are immutable and are structurally
refused by ``reference.prior_art_dir`` / ``reference.assert_write_target``.
"""
