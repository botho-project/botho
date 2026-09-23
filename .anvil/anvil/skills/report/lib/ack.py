"""Structured-YAML ack-file parser for ``report-promote``.

The ack file is a pure-YAML document the operator creates out of band
to authorize a ``report-promote`` run in non-interactive automation
contexts. It carries a structured ``ack:`` token:

.. code-block:: yaml

    ack:
      report_title: "<exact H1 from report.md>"
      recipient:    "<exact recipient from _project.md>"
      sha256:       "<lowercase hex sha256 of report.pdf>"

The skill rejects the prior substring-quoting contract (v0.0.1+ hard
break; anvil is alpha with no shipped consumers). See
``anvil/skills/report/commands/report-promote.md`` step 6 for the full
contract, including the nine enumerated ``AckError.mode`` failure
modes — this module is the executable specification of those modes
(the command doc's prose names eight top-level checks; ``sha256``
mismatch splits into a fresh-file and a stale-file variant, and a
ninth mode covers a stale ack file whose ``sha256`` still matches).

Top-level keys other than ``ack`` are ignored (operators MAY add
workflow fields like ``signature:``, ``signed_by:``, ``notes:`` without
schema churn). Unknown keys *under* ``ack:`` are rejected (typos like
``report-title`` or ``sha-256`` must fail closed).

Each failure mode raises :class:`AckError` with a specific message —
the operator must see *which* check failed without guessing.

``report-promote`` invokes this module via its CLI shim (``python -m
anvil.skills.report.lib.ack``, see ``main`` below) rather than
re-deriving the algorithm in prose — see ``report-promote.md`` step 6
for the exact invocation and its documented last-resort manual
fallback.
"""

from __future__ import annotations

import hashlib
import json
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

import yaml


_REQUIRED_SUBKEYS: tuple[str, ...] = ("report_title", "recipient", "sha256")
_MAX_ACK_AGE_SECONDS: int = 24 * 60 * 60  # 24h defense-in-depth window


class AckError(Exception):
    """Raised when the ack file fails any of the nine contract checks.

    Each instance carries a ``mode`` attribute naming the failure mode
    (one of the nine values this module raises — see the module
    docstring — corresponding to the eight checks enumerated in
    ``report-promote.md`` step 6, with the ``sha256`` mismatch check
    split into fresh-file and stale-file variants) so callers (the
    promoter command, tests, and any future structured-log consumer)
    can dispatch on the mode without parsing the message.
    """

    def __init__(self, mode: str, message: str) -> None:
        super().__init__(message)
        self.mode = mode


@dataclass(frozen=True)
class Ack:
    """The validated, structured ack token extracted from the file."""

    report_title: str
    recipient: str
    sha256: str


def compute_pdf_sha256(pdf_path: Path) -> str:
    """Compute the lowercase-hex sha256 of a PDF's on-disk content.

    Equivalent to ``sha256sum <path> | awk '{print $1}'``.
    """
    return hashlib.sha256(Path(pdf_path).read_bytes()).hexdigest()


def parse_ack_file(
    ack_path: Path,
    *,
    expected_title: str,
    expected_recipient: str,
    expected_sha256: str,
    now: float | None = None,
    max_age_seconds: int = _MAX_ACK_AGE_SECONDS,
) -> Ack:
    """Parse + validate an ack file against the expected promotion context.

    Args:
        ack_path: Path to the YAML ack file the operator created.
        expected_title: The exact H1 read from ``report.md``.
        expected_recipient: The exact recipient string from
            ``_project.md``.
        expected_sha256: The sha256 of ``report.pdf`` computed at
            promotion time (lowercase hex).
        now: Override for the current unix timestamp (testing).
        max_age_seconds: Defense-in-depth mtime window (24h by default).

    Returns:
        The validated :class:`Ack` on success.

    Raises:
        AckError: On any of the nine enumerated failure modes. The
            instance's ``mode`` attribute names the mode; the message
            is operator-facing.
    """
    path = Path(ack_path)

    # Mode 1: file not found.
    if not path.exists():
        raise AckError(
            "file_not_found",
            f"ack file not found: {path}",
        )

    raw = path.read_text()

    # Mode 2: YAML parse error.
    try:
        doc = yaml.safe_load(raw)
    except yaml.YAMLError as exc:
        raise AckError(
            "yaml_parse_error",
            f"ack file is not valid YAML ({path}): {exc}",
        ) from exc

    # Mode 3: missing ack: key.
    if not isinstance(doc, Mapping) or "ack" not in doc:
        raise AckError(
            "missing_ack_key",
            f"ack file is missing the required top-level 'ack:' key: {path}",
        )

    ack_block: Any = doc["ack"]
    if not isinstance(ack_block, Mapping):
        # Treat "ack:" with non-mapping body as a missing-ack-key case
        # (the structured token is absent in any meaningful sense).
        raise AckError(
            "missing_ack_key",
            (
                f"ack file's 'ack:' value must be a mapping with "
                f"report_title / recipient / sha256 subkeys: {path}"
            ),
        )

    # Mode 4: missing required subkey.
    for required in _REQUIRED_SUBKEYS:
        if required not in ack_block:
            raise AckError(
                "missing_required_subkey",
                (
                    f"ack file is missing required 'ack.{required}' "
                    f"subkey: {path}"
                ),
            )

    # Mode 5: unknown subkey under ack: (catches typos like
    # 'report-title', 'sha-256', 'title' — fail closed).
    unknown_subkeys = [
        k for k in ack_block.keys() if k not in _REQUIRED_SUBKEYS
    ]
    if unknown_subkeys:
        raise AckError(
            "unknown_subkey",
            (
                f"ack file has unknown key(s) under 'ack:': "
                f"{sorted(unknown_subkeys)!r}. Allowed keys are "
                f"{list(_REQUIRED_SUBKEYS)!r}. {path}"
            ),
        )

    report_title = _as_str(ack_block["report_title"]).strip()
    recipient = _as_str(ack_block["recipient"]).strip()
    sha256 = _as_str(ack_block["sha256"]).strip()

    # Mode 6: report_title mismatch.
    if report_title != expected_title.strip():
        raise AckError(
            "report_title_mismatch",
            (
                f"ack 'report_title' does not match the report.md H1.\n"
                f"  expected: {expected_title.strip()!r}\n"
                f"  ack file: {report_title!r}"
            ),
        )

    # Mode 7: recipient mismatch.
    if recipient != expected_recipient.strip():
        raise AckError(
            "recipient_mismatch",
            (
                f"ack 'recipient' does not match the _project.md recipient.\n"
                f"  expected: {expected_recipient.strip()!r}\n"
                f"  ack file: {recipient!r}"
            ),
        )

    # Mode 8: sha256 mismatch (with modtime > 24h getting its own
    # rider so the operator's first fix is usually obvious — regenerate
    # the ack file against the fresh PDF).
    if sha256 != expected_sha256.strip().lower():
        ts = now if now is not None else time.time()
        try:
            age = ts - path.stat().st_mtime
        except OSError:
            age = 0.0
        if age > max_age_seconds:
            raise AckError(
                "sha256_mismatch_stale",
                (
                    f"ack 'sha256' does not match the current report.pdf "
                    f"digest, AND the ack file is stale "
                    f"(mtime > {max_age_seconds}s ago). Regenerate the "
                    f"ack file against the fresh PDF and re-promote.\n"
                    f"  expected: {expected_sha256.strip().lower()!r}\n"
                    f"  ack file: {sha256!r}"
                ),
            )
        raise AckError(
            "sha256_mismatch",
            (
                f"ack 'sha256' does not match the current report.pdf "
                f"digest.\n"
                f"  expected: {expected_sha256.strip().lower()!r}\n"
                f"  ack file: {sha256!r}"
            ),
        )

    # Modtime defense-in-depth (separate stale-file mode for cases where
    # sha256 happens to still match but the ack is older than the
    # window — guards against the "operator left an ack file lying
    # around from a prior cycle" footgun).
    ts = now if now is not None else time.time()
    try:
        age = ts - path.stat().st_mtime
    except OSError:
        age = 0.0
    if age > max_age_seconds:
        raise AckError(
            "stale_ack_file",
            (
                f"ack file is older than {max_age_seconds}s "
                f"(age={age:.0f}s). Regenerate it within the 24h window "
                f"and re-promote: {path}"
            ),
        )

    return Ack(
        report_title=report_title,
        recipient=recipient,
        sha256=sha256,
    )


def _as_str(value: Any) -> str:
    """Coerce a YAML scalar to a string for comparison.

    YAML may load a bare ``true`` / ``42`` as a non-string scalar; the
    contract is a string, so we coerce defensively rather than crash.
    """
    return value if isinstance(value, str) else str(value)


# ---------------------------------------------------------------------------
# CLI entry point (non-Python-driver sessions — issue #1098)
# ---------------------------------------------------------------------------
#
# Mirrors the ``if __name__ == "__main__":`` precedent shipped by the other
# ``anvil/lib/*.py`` / ``anvil/skills/*/lib/*.py`` modules (e.g.
# ``anvil/lib/pending_marker.py``, ``anvil/lib/sidecar.py``,
# ``anvil/skills/report/lib/claim_figure_grounding.py``). A manual or agent
# promotion session with no orchestrating Python driver can shell out to
# ``python -m anvil.skills.report.lib.ack`` and get the exact same
# nine-mode validation this module's Python API enforces, rather than
# re-deriving the algorithm in prose each run. ``report-promote.md`` step 6
# is the sole documented caller.


def _build_cli_parser():
    import argparse

    p = argparse.ArgumentParser(
        prog="python -m anvil.skills.report.lib.ack",
        description=(
            "Parse and validate a report-promote ack file against the "
            "expected promotion context. Exit 0 with the validated "
            "report_title/recipient/sha256 as JSON on stdout when the ack "
            "file passes all nine checks; exit 1 with a mode-specific "
            "error on stderr (`error (mode=<mode>): <message>`) on the "
            "first failing check. See report-promote.md step 6 for the "
            "full contract."
        ),
    )
    p.add_argument(
        "ack_file",
        help="Path to the operator-authored YAML ack file.",
    )
    p.add_argument(
        "--expected-title",
        required=True,
        help="The exact H1 heading read from report.md.",
    )
    p.add_argument(
        "--expected-recipient",
        required=True,
        help="The exact recipient field from _project.md.",
    )
    p.add_argument(
        "--expected-sha256",
        required=True,
        help="The sha256 (lowercase hex) of report.pdf at promotion time.",
    )
    p.add_argument(
        "--max-age-seconds",
        type=int,
        default=_MAX_ACK_AGE_SECONDS,
        help=(
            "Defense-in-depth mtime window for the ack file, in seconds "
            "(default: 24h)."
        ),
    )
    return p


def main(argv: "Sequence[str] | None" = None) -> int:
    """CLI entry point. Returns the process exit code.

    Exit codes:

    - ``0``: the ack file passed all nine checks. The validated
      ``report_title`` / ``recipient`` / ``sha256`` are printed to
      stdout as JSON.
    - ``1``: the ack file failed one of the nine checks. The specific
      ``AckError.mode`` and its operator-facing message are printed to
      stderr as ``error (mode=<mode>): <message>``.

    Invocation errors (missing required flags) are handled by
    ``argparse`` itself, which exits ``2``.
    """
    parser = _build_cli_parser()
    args = parser.parse_args(argv)
    try:
        ack = parse_ack_file(
            Path(args.ack_file),
            expected_title=args.expected_title,
            expected_recipient=args.expected_recipient,
            expected_sha256=args.expected_sha256,
            max_age_seconds=args.max_age_seconds,
        )
    except AckError as exc:
        print(f"error (mode={exc.mode}): {exc}", file=sys.stderr)
        return 1
    print(
        json.dumps(
            {
                "report_title": ack.report_title,
                "recipient": ack.recipient,
                "sha256": ack.sha256,
            },
            indent=2,
        )
    )
    return 0


__all__ = [
    "AckError",
    "Ack",
    "compute_pdf_sha256",
    "parse_ack_file",
    "main",
]


if __name__ == "__main__":
    raise SystemExit(main())
