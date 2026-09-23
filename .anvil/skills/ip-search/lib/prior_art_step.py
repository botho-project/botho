"""Opt-in pre-critic prior-art search step for the ip suite (issue #958).

`anvil:ip-search` (issue #957) shipped the searcher. Issue #975 wired an
opt-in, off-by-default trigger for it into the `ip-uspto` /
`ip-uspto-provisional` prior-art critics — a `--search` CLI flag and/or
`<thread>/.anvil.json`'s `prior_art_search.auto` knob — but shipped it as
**prose only**: the four safety properties below were documented in the
critic command files and enforced only by an LLM-driven agent correctly
following that prose at runtime.

This module (issue #978, backported from the #977 prototype) is the code
those properties now run through. It resolves the same knob #975 already
shipped and, only when the knob is set, runs `ip-search` immediately ahead
of the prior-art critic so the positioning critic scores against
freshly-searched art instead of whatever happened to be hand-collected.

**Config shape matches #975's shipped contract exactly** — this is a
deliberate compatibility decision, not an oversight: any thread whose
`.anvil.json` already carries `{"prior_art_search": {"auto": true, ...}}`
must keep opting in after this backport lands, and the documented
`--search` flag / `auto` / `corpus` / `max` / `min_score` keys /
`auto`/8/1 defaults are unchanged. Two differences from the #977 prototype
this was adapted from, both intentional:

- The enable key is `auto` (not `enabled`) and the count key is `max` (not
  `max_references`), matching #975's shipped docs verbatim.
- The knob is **object-only** — `{"prior_art_search": true}` (bare bool)
  is not accepted. #975 never documented a bool-shorthand form ("Only
  ``auto: true`` is required to opt in" implies the object form), and
  #977's additive `query` option is dropped for the same reason: neither
  is part of the shipped, documented contract. Both a bare bool and a
  `query` key are simply unrecognized/ignored inputs here, not new surface
  — if either is wanted later it is a separate, documented decision.

Four properties are load-bearing, and each is enforced here in code rather
than left to the command doc's prose:

1. **Off by default.** Absent a `prior_art_search` key in
   ``<thread>/.anvil.json`` (and absent a CLI override), :func:`run_step`
   returns ``state="disabled"`` **without calling the runner at all** — no
   import of a corpus client, no network call, no API-key read, no cost. A
   malformed/unreadable ``.anvil.json`` also leaves the step off (fail-safe:
   a config the parser cannot understand must never *enable* network access).
2. **Never clobbers operator work.** ``force`` is pinned to ``False`` at the
   call site and is not exposed as a knob option; a caller that passes one
   anyway is refused with a warning. `ip-search` already skips a patent an
   existing file covers and suffixes a colliding slug, so a re-run of the
   step is idempotent over hand-authored ``prior-art/*.md``.
3. **Machine-fetched art is distinguishable.** Every file `ip-search` writes
   carries ``source: "anvil:ip-search/<corpus>"`` frontmatter;
   :func:`partition_prior_art` turns that into a mechanical
   (machine, operator) split so a human — and the step report — can always
   tell the two apart.
4. **Never blocks.** A degraded search (no API key, unreachable corpus), an
   input error, or an unavailable `ip-search` install are all reported and
   stepped over. The prior-art critic then runs exactly as it does today,
   including the "no prior art supplied → Dim 5 ``null``" path.

The step writes nothing itself: the only on-disk effect is whatever
`ip-search` writes under ``<thread>/prior-art/``.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, List, Mapping, Optional, Tuple

from .orchestrate import (
    DEFAULT_MAX_REFERENCES,
    DEFAULT_MIN_SCORE,
    SearchRun,
)
from .orchestrate import run as _default_runner

#: The per-thread config file the ip skills already read for
#: ``max_iterations`` / ``critics`` overrides.
CONFIG_FILENAME = ".anvil.json"

#: The knob. A sibling top-level key alongside ``max_iterations`` /
#: ``critics``; absent ⇒ the step is off. Matches #975's shipped key.
KNOB_KEY = "prior_art_search"

#: Frontmatter marker every `ip-search`-written reference carries.
PROVENANCE_PREFIX = "anvil:ip-search"

_VALID_CORPORA = ("auto", "patentsview", "uspto")

#: Options an operator may set on the knob's object form — exactly #975's
#: shipped keys. ``force`` is deliberately absent — see the module
#: docstring, property 2.
_KNOB_OPTIONS = ("auto", "corpus", "max", "min_score")

#: Terminal states. Only ``ok`` means art may have been added; every other
#: state leaves the thread exactly as the step found it.
STATE_DISABLED = "disabled"
STATE_OK = "ok"
STATE_DEGRADED = "degraded"
STATE_ERROR = "error"
STATE_UNAVAILABLE = "unavailable"


class PriorArtStepError(ValueError):
    """Raised only for a programming error in the call site itself.

    Config problems are warnings that leave the step off; corpus problems
    are a degraded run. Neither raises. This exists for the one case the
    contract cannot absorb: a caller trying to force-overwrite through the
    lifecycle step.
    """


# ---------------------------------------------------------------------------
# Config resolution
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class StepConfig:
    """The resolved knob for one thread.

    Field names mirror #975's shipped JSON keys exactly: ``auto`` (not
    ``enabled``), ``max`` (not ``max_references``). ``origin`` records
    where the decision came from — ``"default"`` (no knob anywhere),
    ``"thread-config"`` (``<thread>/.anvil.json``), or ``"cli"`` (an
    explicit per-invocation override) — so the step report can say *why*
    it did or did not search.
    """

    auto: bool = False
    corpus: str = "auto"
    max: int = DEFAULT_MAX_REFERENCES
    min_score: int = DEFAULT_MIN_SCORE
    origin: str = "default"
    warnings: Tuple[str, ...] = ()

    def describe(self) -> str:
        if not self.auto:
            return f"off ({self.origin})"
        return f"on ({self.origin}; corpus={self.corpus}, max={self.max})"


def _read_config(path: Path) -> Tuple[Mapping[str, Any], List[str]]:
    """Read ``.anvil.json`` leniently.

    A missing file is the common case (no warning). An unreadable or
    malformed one is a warning and an empty mapping — which keeps the step
    **off**, matching the fail-safe rule.
    """

    if not path.is_file():
        return {}, []
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as exc:  # pragma: no cover - platform-dependent
        return {}, [f"{path.name} unreadable ({exc}); prior-art search step stays off"]
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        return {}, [
            (
                f"{path.name} is not valid JSON ({exc.msg} at line "
                f"{exc.lineno}); prior-art search step stays off"
            )
        ]
    if not isinstance(data, dict):
        return {}, [
            (
                f"{path.name} does not contain a JSON object; "
                f"prior-art search step stays off"
            )
        ]
    return data, []


def _coerce_int(
    value: Any, name: str, default: int, minimum: int, warnings: List[str]
) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        warnings.append(
            f"{KNOB_KEY}.{name} must be an integer (got {value!r}); "
            f"using the default {default}"
        )
        return default
    if value < minimum:
        warnings.append(
            f"{KNOB_KEY}.{name} must be >= {minimum} (got {value}); "
            f"using the default {default}"
        )
        return default
    return value


def _config_from_mapping(raw: Mapping[str, Any]) -> Tuple[dict, List[str]]:
    """Validate the knob's object form into :class:`StepConfig` kwargs.

    Only #975's shipped keys (``auto`` / ``corpus`` / ``max`` /
    ``min_score``) are recognized. Anything else — including the #977
    prototype's ``enabled`` / ``max_references`` / ``query`` names, or a
    stray ``force`` — is an unrecognized-option warning, not a silent
    rename; a thread config written against the shipped docs never hits
    this path.
    """

    warnings: List[str] = []
    kwargs: dict = {}

    for key in raw:
        if key == "force":
            warnings.append(
                f"{KNOB_KEY}.force is not an accepted option — the lifecycle "
                f"step never overwrites collected references; ignoring it "
                f"(run `/anvil:ip-search <thread> --force` by hand if that is "
                f"really what you want)"
            )
        elif key not in _KNOB_OPTIONS:
            warnings.append(f"{KNOB_KEY}.{key} is not a recognized option; ignoring it")

    # Unlike the enable-by-presence #977 prototype, ``auto`` must be set
    # explicitly true to opt in — matching the shipped doc's "Only
    # `auto: true` is required to opt in" literally: the object form alone
    # (e.g. just `{"corpus": "patentsview"}`) does not imply enablement.
    auto = raw.get("auto", False)
    if not isinstance(auto, bool):
        warnings.append(
            f"{KNOB_KEY}.auto must be true or false (got {auto!r}); "
            f"prior-art search step stays off"
        )
        auto = False
    kwargs["auto"] = auto

    corpus = raw.get("corpus", "auto")
    if corpus not in _VALID_CORPORA:
        warnings.append(
            f"{KNOB_KEY}.corpus must be one of {', '.join(_VALID_CORPORA)} "
            f"(got {corpus!r}); using 'auto'"
        )
        corpus = "auto"
    kwargs["corpus"] = corpus

    kwargs["max"] = _coerce_int(
        raw.get("max", DEFAULT_MAX_REFERENCES),
        "max",
        DEFAULT_MAX_REFERENCES,
        1,
        warnings,
    )
    kwargs["min_score"] = _coerce_int(
        raw.get("min_score", DEFAULT_MIN_SCORE),
        "min_score",
        DEFAULT_MIN_SCORE,
        0,
        warnings,
    )
    return kwargs, warnings


def resolve_config(
    thread_dir: Path,
    cli_enable: Optional[bool] = None,
    config: Optional[Mapping[str, Any]] = None,
) -> StepConfig:
    """Resolve the opt-in knob for ``thread_dir``.

    Args:
        thread_dir: the THREAD root (the dir holding ``BRIEF.md``,
            ``prior-art/``, and ``.anvil.json``).
        cli_enable: an explicit per-invocation override — ``True`` for the
            documented ``--search`` flag, ``False`` to suppress a
            configured thread for one run, ``None`` (the default) to defer
            to the thread config. The CLI wins over the file in both
            directions, so a one-off run can enable a search on an
            unconfigured thread *and* suppress one on a configured thread.
        config: pre-parsed config mapping (test seam); when given,
            ``.anvil.json`` is not read.

    Returns:
        A :class:`StepConfig`. Never raises — every config problem is a
        warning that leaves the step **off**.
    """

    warnings: List[str] = []
    if config is None:
        data, read_warnings = _read_config(Path(thread_dir) / CONFIG_FILENAME)
        warnings.extend(read_warnings)
    else:
        data = config

    raw = data.get(KNOB_KEY)
    origin = "default"
    kwargs: dict = {}

    if raw is None:
        pass
    elif isinstance(raw, dict):
        origin = "thread-config"
        kwargs, knob_warnings = _config_from_mapping(raw)
        warnings.extend(knob_warnings)
    else:
        # #975's shipped contract documents the object form only — a bare
        # bool (or any other type) is unrecognized, not an alternate
        # shorthand, so it fails safe to off rather than silently doing
        # something the docs never described.
        warnings.append(
            f"{KNOB_KEY} must be an object (got {raw!r}); prior-art search "
            f"step stays off"
        )

    if cli_enable is not None:
        kwargs["auto"] = bool(cli_enable)
        origin = "cli"

    kwargs.setdefault("auto", False)
    return StepConfig(origin=origin, warnings=tuple(warnings), **kwargs)


# ---------------------------------------------------------------------------
# Provenance partition
# ---------------------------------------------------------------------------


def _frontmatter_lines(text: str) -> List[str]:
    """The lines of a leading ``---``-delimited frontmatter block, if any."""

    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return []
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            return lines[1:i]
    return []


def is_machine_fetched(path: Path) -> bool:
    """True when ``path`` carries the `ip-search` provenance marker.

    The marker is the ``source: "anvil:ip-search/<corpus>"`` frontmatter key
    every emitted reference carries. Only the leading frontmatter block is
    authoritative — a hand-authored reference that merely *mentions*
    `ip-search` in its body stays correctly attributed to its operator.
    Reading the marker rather than inferring from a filename also means an
    operator may rename a machine-fetched file without losing its provenance.
    """

    try:
        text = Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False
    for line in _frontmatter_lines(text):
        stripped = line.strip()
        if stripped.startswith("source:") and PROVENANCE_PREFIX in stripped:
            return True
    return False


def partition_prior_art(thread_dir: Path) -> Tuple[List[Path], List[Path]]:
    """Split ``<thread>/prior-art/*.md`` into (machine-fetched, operator).

    The mechanical answer to "which of these did the tool write?" — used by
    the step report so the operator never has to guess, and by the
    idempotence tests so "did not clobber operator art" is checkable rather
    than asserted.
    """

    art_dir = Path(thread_dir) / "prior-art"
    machine: List[Path] = []
    operator: List[Path] = []
    if not art_dir.is_dir():
        return machine, operator
    for path in sorted(art_dir.glob("*.md")):
        (machine if is_machine_fetched(path) else operator).append(path)
    return machine, operator


# ---------------------------------------------------------------------------
# The step
# ---------------------------------------------------------------------------


@dataclass
class StepResult:
    """Outcome of one pre-critic search step.

    ``blocking`` is a constant ``False``: this step is a *supply* stage, not
    a gate. Whatever happens here, the prior-art critic runs next.
    """

    state: str
    thread: str
    config: StepConfig
    search: Optional[SearchRun] = None
    written: List[Path] = field(default_factory=list)
    preserved: List[Path] = field(default_factory=list)
    machine_fetched: List[Path] = field(default_factory=list)
    operator_authored: List[Path] = field(default_factory=list)
    warnings: List[str] = field(default_factory=list)
    report: str = ""

    @property
    def ran(self) -> bool:
        """True when a corpus search was actually attempted."""

        return self.state != STATE_DISABLED

    @property
    def blocking(self) -> bool:
        """Always False — the step never gates the prior-art critic."""

        return False

    @property
    def art_available(self) -> bool:
        """True when the critic will find at least one reference to score.

        The bridge to the "no prior art supplied → Dim 5 ``null``" path: when
        this is False the critic behaves exactly as it does today.
        """

        return bool(self.machine_fetched or self.operator_authored)


def _render_report(result: StepResult) -> str:
    cfg = result.config
    lines = [f"# prior-art search step — {result.thread}", ""]
    lines.append(f"- **Knob** (`{KNOB_KEY}`): {cfg.describe()}")
    lines.append(f"- **State**: {result.state}")

    if result.state == STATE_DISABLED:
        lines.append(
            f"- No search was attempted: no corpus query, no API-key read, "
            f"no network call. Set `{{\"{KNOB_KEY}\": {{\"auto\": true}}}}` "
            f"in `<thread>/{CONFIG_FILENAME}` (or pass `--search`) to "
            f"enable it."
        )
    else:
        lines.append(f"- **References written**: {len(result.written)}")
        lines.append(
            f"- **Already collected (left untouched)**: {len(result.preserved)}"
        )

    lines.append("")
    lines.append("## Prior-art dir after the step")
    lines.append("")
    lines.append(
        f"- machine-fetched (`source: {PROVENANCE_PREFIX}/…`): "
        f"{len(result.machine_fetched)}"
    )
    lines.append(f"- operator-authored: {len(result.operator_authored)}")
    if not result.art_available:
        lines.append("")
        lines.append(
            "No references are present, so the prior-art critic will score "
            "Dimension 5 `null` (\"no prior art supplied\") exactly as it "
            "does on a thread that never ran this step."
        )
    lines.append("")

    if result.warnings:
        lines.append("## Warnings")
        lines.append("")
        lines.extend(f"- {w}" for w in result.warnings)
        lines.append("")

    lines.append(
        "This step never blocks: the prior-art critic runs next regardless "
        "of the outcome above. `anvil:ip-search` is a drafting aid, not a "
        "professional or attorney clearance search."
    )
    lines.append("")
    return "\n".join(lines)


def run_step(
    thread_dir: Path,
    cli_enable: Optional[bool] = None,
    config: Optional[StepConfig] = None,
    runner: Optional[Callable[..., SearchRun]] = None,
    runner_kwargs: Optional[Mapping[str, Any]] = None,
) -> StepResult:
    """Run the opt-in prior-art search immediately before the critic.

    Args:
        thread_dir: the THREAD root.
        cli_enable: per-invocation override (see :func:`resolve_config`) —
            the documented ``--search`` flag maps to ``cli_enable=True``.
        config: a pre-resolved :class:`StepConfig` (test seam / caller that
            already resolved the knob).
        runner: the search entry point; defaults to
            ``ip-search``'s ``orchestrate.run``. Injected by tests so the
            suite never touches the network.
        runner_kwargs: extra keyword arguments forwarded to the runner —
            the ``env`` / ``opener`` / ``clock`` injection seams. Passing
            ``force`` is refused.

    Returns:
        A :class:`StepResult`. Never raises for a search-side failure: a
        missing key, an unreachable corpus, a bad brief, or an `ip-search`
        install that cannot be loaded all come back as a reported state the
        caller steps over.
    """

    thread_path = Path(thread_dir)
    thread_name = thread_path.resolve().name
    cfg = config if config is not None else resolve_config(thread_path, cli_enable)

    result = StepResult(state=STATE_DISABLED, thread=thread_name, config=cfg)
    result.warnings.extend(cfg.warnings)

    extra = dict(runner_kwargs or {})
    if "force" in extra:
        raise PriorArtStepError(
            "the lifecycle prior-art search step never overwrites collected "
            "references; `force` is not an accepted argument. Run "
            "`/anvil:ip-search <thread> --force` by hand instead."
        )

    if cfg.auto:
        call = runner if runner is not None else _default_runner
        try:
            search = call(
                thread_path,
                corpus=cfg.corpus,
                max_references=cfg.max,
                min_score=cfg.min_score,
                # Pinned, not configurable: an operator's hand-annotated
                # reference file must survive every lifecycle re-run.
                force=False,
                **extra,
            )
        except Exception as exc:  # noqa: BLE001 - the step must never raise
            result.state = STATE_UNAVAILABLE
            result.warnings.append(
                f"prior-art search step could not run ({type(exc).__name__}: "
                f"{exc}); continuing to the prior-art critic with the art "
                f"already on disk"
            )
        else:
            result.search = search
            result.written = list(search.written)
            result.preserved = list(search.skipped)
            result.warnings.extend(search.warnings)
            result.state = {
                "ok": STATE_OK,
                "degraded": STATE_DEGRADED,
                "error": STATE_ERROR,
            }.get(search.status, STATE_ERROR)

    result.machine_fetched, result.operator_authored = partition_prior_art(thread_path)
    result.report = _render_report(result)
    return result


def next_command(skill: str, thread: str) -> str:
    """The command the caller runs after this step, for the report line."""

    return f"{skill}-prior-art {thread}"


def summary_line(result: StepResult, skill: str) -> str:
    """One-line console summary, in the ip suite's report idiom."""

    if result.state == STATE_DISABLED:
        head = f"prior-art search step: off ({KNOB_KEY} not set)"
    elif result.state == STATE_OK:
        head = (
            f"prior-art search step: {len(result.written)} written, "
            f"{len(result.preserved)} already collected"
        )
    elif result.state == STATE_DEGRADED:
        head = "prior-art search step: degraded (no corpus reachable — nothing written)"
    elif result.state == STATE_UNAVAILABLE:
        head = "prior-art search step: unavailable (anvil:ip-search not runnable)"
    else:
        head = "prior-art search step: error (see warnings) — nothing written"
    art = (
        f"{len(result.machine_fetched)} machine + "
        f"{len(result.operator_authored)} operator reference(s) on disk"
    )
    return f"{head}; {art}; next: {next_command(skill, result.thread)}"


__all__ = [
    "CONFIG_FILENAME",
    "KNOB_KEY",
    "PROVENANCE_PREFIX",
    "STATE_DEGRADED",
    "STATE_DISABLED",
    "STATE_ERROR",
    "STATE_OK",
    "STATE_UNAVAILABLE",
    "PriorArtStepError",
    "StepConfig",
    "StepResult",
    "is_machine_fetched",
    "next_command",
    "partition_prior_art",
    "resolve_config",
    "run_step",
    "summary_line",
]
