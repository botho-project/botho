"""Load the rubric-rebackport ``lib/`` package under a unique module name.

This module exists to dodge the cross-skill ``lib`` package name
collision that occurs when multiple per-skill test suites each ship
their own ``lib/`` package (e.g., ``project-migrate``'s tests cache
``lib`` in ``sys.modules``, then ``rubric-rebackport``'s tests can't
import their own ``lib.detect``).

Delegates the actual loading to the shared
``anvil.lib.skill_lib_loader`` helper (issue #879) rather than
re-deriving the ``importlib`` incantation here — that module registers
``lib/`` under the unique package name ``rubric_rebackport_lib`` and
resolves each requested submodule's relative imports (``from .detect
import ...``) via normal import machinery, so no hand-maintained load
order is needed. The loaded modules are exposed as attributes on this
module so tests can write ``from _skill_lib import detect, plan, ...``.
"""

from __future__ import annotations

from pathlib import Path

from anvil.lib.skill_lib_loader import load_skill_lib

_HERE = Path(__file__).resolve().parent
_SKILL_ROOT = _HERE.parent
_LIB_DIR = _SKILL_ROOT / "lib"

_PACKAGE_NAME = "rubric_rebackport_lib"

_lib = load_skill_lib(
    "rubric-rebackport",
    _LIB_DIR,
    ["detect", "plan", "stamp", "rescore", "apply", "verify", "orchestrate"],
    package_name=_PACKAGE_NAME,
)

# Re-export each submodule on this helper.
detect = _lib.detect
plan = _lib.plan
stamp = _lib.stamp
rescore = _lib.rescore
apply_mod = _lib.apply
verify = _lib.verify
orchestrate = _lib.orchestrate


__all__ = [
    "apply_mod",
    "detect",
    "orchestrate",
    "plan",
    "rescore",
    "stamp",
    "verify",
]
