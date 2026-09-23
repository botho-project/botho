"""Programmatic fixtures for the `anvil:ip-search` suite (issue #957).

Two kinds of fixture live here:

- **Thread builders** — write a minimal but realistic thread on disk
  (``BRIEF.md`` with a ``§3 — Inventive features`` inventory, optional
  version dirs / critic siblings so write-scope tests have something
  immutable to aim at).
- **Cassette openers** — a fake ``opener`` (``(Request, timeout) -> bytes``)
  that returns recorded corpus JSON from ``tests/cassettes/`` instead of
  touching the network. Every corpus test in this suite goes through one;
  there is no live-network path in the default run.
"""

from __future__ import annotations

import json
import urllib.error
from pathlib import Path
from typing import Callable, Iterable, List, Optional

CASSETTES = Path(__file__).resolve().parent / "cassettes"


# ---------------------------------------------------------------------------
# Thread builders
# ---------------------------------------------------------------------------

BRIEF_TEXT = """---
title: Passive Thermal Compensation Network for a Piezoresistive Pressure Sensor
inventors:
  - Dana R. Okoye
artifact_type: ip-uspto-provisional
---

# Inventor Brief — Passive Thermal Compensation

## §1 — Problem

Piezoresistive MEMS pressure sensors drift with temperature.

## §2 — Prior approaches (do NOT admit as prior art in the spec Background)

- Digital compensation with a microcontroller and a lookup table.

## §3 — Inventive features (the disclosure denominator)

3.1 **Split-path excitation network.** The bridge supply is divided into a
constant-current leg and a proportional-to-absolute-temperature (PTAT) leg
whose ratio is set by a single ratio resistor.

3.2 **Self-referencing offset-cancellation node.** A compensation tap is
taken from the midpoint of a matched dummy half-bridge fabricated on the
same die, adjacent to the sense bridge.

## §4 — Embodiments

4.1 **Discrete-resistor embodiment.** All compensation elements are
discrete surface-mount thin-film resistors.
"""


def make_thread(
    root: Path, slug: str = "acme-widget-prov", brief: Optional[str] = None
) -> Path:
    """Create ``<root>/<slug>/`` with a BRIEF.md and return the thread dir."""

    thread = Path(root) / slug
    thread.mkdir(parents=True, exist_ok=True)
    (thread / "BRIEF.md").write_text(
        BRIEF_TEXT if brief is None else brief, encoding="utf-8"
    )
    return thread


def make_version_dir(root: Path, slug: str = "acme-widget-prov", n: int = 1) -> Path:
    """Create an immutable version dir ``<root>/<slug>.{n}/``."""

    version = Path(root) / f"{slug}.{n}"
    version.mkdir(parents=True, exist_ok=True)
    (version / "spec.tex").write_text("% spec\n", encoding="utf-8")
    return version


def make_critic_sibling(
    root: Path, slug: str = "acme-widget-prov", n: int = 1, tag: str = "priorart"
) -> Path:
    """Create an immutable critic sibling ``<root>/<slug>.{n}.<tag>/``."""

    sibling = Path(root) / f"{slug}.{n}.{tag}"
    sibling.mkdir(parents=True, exist_ok=True)
    return sibling


# ---------------------------------------------------------------------------
# Cassette openers
# ---------------------------------------------------------------------------


def load_cassette(name: str) -> bytes:
    """Read a recorded corpus response by filename stem."""

    return (CASSETTES / f"{name}.json").read_bytes()


def cassette_opener(name: str, record: Optional[List] = None) -> Callable:
    """Opener that always returns cassette ``name``.

    ``record`` (when given) collects each ``urllib.request.Request`` the
    code under test built, so a test can assert on the URL, headers, and
    body without any network.
    """

    payload = load_cassette(name)

    def _opener(request, timeout):  # noqa: ANN001 - fake opener signature
        if record is not None:
            record.append(request)
        return payload

    return _opener


def sequence_opener(payloads: Iterable[bytes]) -> Callable:
    """Opener returning each payload in turn (last one repeats)."""

    items = list(payloads)

    def _opener(request, timeout):  # noqa: ANN001
        return items.pop(0) if len(items) > 1 else items[0]

    return _opener


def raising_opener(exc: BaseException) -> Callable:
    """Opener that always raises ``exc`` (transport / auth failures)."""

    def _opener(request, timeout):  # noqa: ANN001
        raise exc

    return _opener


def http_error(code: int, url: str = "https://example.invalid/") -> urllib.error.HTTPError:
    """Build an ``HTTPError`` with ``code`` for degradation tests."""

    return urllib.error.HTTPError(url, code, "test", {}, None)


def raw_opener(payload: bytes) -> Callable:
    """Opener returning arbitrary bytes (malformed-response tests)."""

    def _opener(request, timeout):  # noqa: ANN001
        return payload

    return _opener


def json_opener(payload: dict) -> Callable:
    """Opener returning ``payload`` serialized as JSON."""

    return raw_opener(json.dumps(payload).encode("utf-8"))


def no_sleep(_seconds: float) -> None:
    """Drop-in for ``time.sleep`` so retry tests do not actually wait."""


def fixed_clock() -> float:
    """A frozen epoch (2026-01-15T00:00:00Z) for deterministic dates."""

    return 1768435200.0


__all__ = [
    "BRIEF_TEXT",
    "CASSETTES",
    "cassette_opener",
    "fixed_clock",
    "http_error",
    "json_opener",
    "load_cassette",
    "make_critic_sibling",
    "make_thread",
    "make_version_dir",
    "no_sleep",
    "raising_opener",
    "raw_opener",
    "sequence_opener",
]
