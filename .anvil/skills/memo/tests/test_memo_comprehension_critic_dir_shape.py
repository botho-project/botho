"""Discovery + aggregation tests for the memo comprehension critic sibling (issue #753).

The comprehension critic (``anvil/skills/memo/commands/memo-comprehension.md``)
is a new opt-in critic sibling at ``<thread>.{N}.comprehension/`` that consumes
the existing canonical ``_review.json`` schema (``anvil/lib/review_schema.py``)
and is discovered by ``anvil/lib/critics.py::discover_critics`` without any
aggregator change. Unlike ``memo-redteam`` (which owns dims 2/3 and emits
critical flags), the comprehension critic is **findings-only**: it owns NO
rubric dimension (all ``score`` entries ``null``) and NEVER emits a critical
flag (``critical_flags`` always empty). Its findings surface at ``major``
severity in ``comments.md``.

The load-bearing property under test: an all-null-scores comprehension sibling
is **provably non-perturbing** to ``aggregate()``. The merged total, per-dim
scores, and verdict are byte-identical whether or not the comprehension sibling
is present, because the aggregator's mean-of-non-null contract drops every
null score. This is what makes the critic additive and non-gating.

Per the per-skill test filename convention (#58 — distinct filenames across
skills, ``__init__.py`` chains in every test dir), this file is named
``test_memo_comprehension_critic_dir_shape.py``.
"""

from __future__ import annotations

import json
from pathlib import Path

from anvil.lib.critics import (
    CANONICAL_REVIEW_FILENAME,
    aggregate,
    discover_critics,
    load_review,
)
from anvil.lib.review_schema import (
    Finding,
    Kind,
    Review,
    Score,
    Verdict,
)


# Per-dim max weights for the memo /44 rubric (dims 1..9).
_DIM_MAXES = {
    "dim_1": 4,
    "dim_2": 6,
    "dim_3": 6,
    "dim_4": 4,
    "dim_5": 6,
    "dim_6": 6,
    "dim_7": 4,
    "dim_8": 4,
    "dim_9": 4,
}


def _make_comprehension_review(
    *,
    version_dir: str = "investment-memo.5",
    with_findings: bool = True,
) -> Review:
    """Build a canonical comprehension Review payload.

    The comprehension critic owns NO dimension: every ``score`` is ``None``.
    The schema still requires a full scorecard (empty ``scores`` is rejected
    for a scored review), so all nine dims are enumerated with ``score=None``.
    ``critical_flags`` is ALWAYS empty. Findings (GARBLED / MISSING / jargon)
    are ``major`` severity.
    """
    scores = [
        Score(dimension=dim, score=None, max=mx)
        for dim, mx in _DIM_MAXES.items()
    ]
    findings = []
    if with_findings:
        findings = [
            Finding(
                severity="major",
                dimension=None,  # cross-cutting — owns no scored dimension
                evidence_span="investment-memo.md:L1-L40",
                rationale=(
                    "Q1 (what does this company sell) is MISSING: a blind "
                    "reader cannot name the product from the body; the memo "
                    "fogs it behind mechanism language ('the substrate under "
                    "the primitive')."
                ),
                suggested_fix=(
                    "State the product in one plain sentence up front."
                ),
            ),
            Finding(
                severity="major",
                dimension=None,
                evidence_span="investment-memo.md:L88-L88",
                rationale=(
                    "Undefined load-bearing term 'f_token' — defined once in "
                    "a subordinate clause, then used ~8 times as the "
                    "protagonist."
                ),
                suggested_fix=(
                    "Define f_token at first use or delete the coinage."
                ),
            ),
        ]

    return Review(
        schema_version="1",
        kind=Kind.JUDGMENT,
        version_dir=version_dir,
        critic_id="comprehension",
        scores=scores,
        findings=findings,
        critical_flags=[],  # comprehension critic NEVER gates
        total=None,
        threshold=35,
        rubric="anvil-memo-v2",
    )


def _make_review_critic(
    *,
    version_dir: str = "investment-memo.5",
    total_clears_threshold: bool = True,
) -> Review:
    """Build a passable memo-review payload (owns all nine dims)."""
    s = 4 if total_clears_threshold else 1
    return Review(
        version_dir=version_dir,
        critic_id="review",
        scores=[
            Score(dimension="dim_1", score=s, max=4),
            Score(dimension="dim_2", score=6 if total_clears_threshold else 2, max=6),
            Score(dimension="dim_3", score=6 if total_clears_threshold else 2, max=6),
            Score(dimension="dim_4", score=s, max=4),
            Score(dimension="dim_5", score=6 if total_clears_threshold else 2, max=6),
            Score(dimension="dim_6", score=6 if total_clears_threshold else 2, max=6),
            Score(dimension="dim_7", score=s, max=4),
            Score(dimension="dim_8", score=s, max=4),
            Score(dimension="dim_9", score=s, max=4),
        ],
        threshold=35,
        rubric="anvil-memo-v2",
    )


def _write_sidecar(critic_dir: Path, review: Review) -> None:
    critic_dir.mkdir(parents=True, exist_ok=True)
    (critic_dir / CANONICAL_REVIEW_FILENAME).write_text(
        review.model_dump_json(indent=2)
    )


def test_discover_finds_comprehension_sibling(tmp_path):
    """discover_critics finds <thread>.{N}.comprehension/ alongside .review/."""
    (tmp_path / "investment-memo.5").mkdir()
    _write_sidecar(tmp_path / "investment-memo.5.review", _make_review_critic())
    _write_sidecar(
        tmp_path / "investment-memo.5.comprehension",
        _make_comprehension_review(),
    )

    siblings = discover_critics(tmp_path / "investment-memo.5")
    names = sorted(s.name for s in siblings)
    assert names == [
        "investment-memo.5.comprehension",
        "investment-memo.5.review",
    ]


def test_load_review_parses_comprehension_review_json(tmp_path):
    """load_review parses a comprehension _review.json without error."""
    comp_dir = tmp_path / "investment-memo.5.comprehension"
    _write_sidecar(comp_dir, _make_comprehension_review())

    loaded = load_review(comp_dir)
    assert loaded.critic_id == "comprehension"
    assert loaded.version_dir == "investment-memo.5"
    # Owns no dimension: every score is null.
    assert all(s.score is None for s in loaded.scores)
    # Full scorecard is still enumerated (schema requires it).
    assert len(loaded.scores) == 9
    # NEVER gates.
    assert loaded.critical_flags == []
    # Findings are major (never blocker — a blocker would imply critical).
    assert loaded.findings
    assert all(f.severity == "major" for f in loaded.findings)


def test_comprehension_all_null_scores_do_not_perturb_aggregate(tmp_path):
    """The comprehension sibling is provably non-perturbing to aggregate().

    The core contract of issue #753's "no score, no gate" requirement: an
    all-null comprehension sibling combined with a real scored .review/
    sibling produces a byte-identical total and verdict to the .review/
    sibling alone. The aggregator's mean-of-non-null drops every null score.
    """
    review = _make_review_critic(total_clears_threshold=True)
    comprehension = _make_comprehension_review()

    agg_review_only = aggregate([review])
    agg_with_comprehension = aggregate([review, comprehension])

    # Total unchanged.
    assert agg_with_comprehension.total == agg_review_only.total
    # Verdict unchanged (still ADVANCE — the comprehension critic never
    # emits a critical flag, so it cannot force BLOCK).
    assert agg_review_only.verdict == Verdict.ADVANCE
    assert agg_with_comprehension.verdict == Verdict.ADVANCE
    # Per-dimension merged scores unchanged.
    means_review = {s.dimension: s.score for s in agg_review_only.scores}
    means_with = {s.dimension: s.score for s in agg_with_comprehension.scores}
    assert means_review == means_with
    # No critical flags contributed.
    assert agg_with_comprehension.critical_flags == []


def test_comprehension_does_not_rescue_a_failing_review(tmp_path):
    """A sub-threshold review stays REVISE even with a comprehension sibling.

    Symmetric guard to the non-perturbation test: the comprehension critic
    contributes nothing that could lift a failing total over threshold, so a
    REVISE verdict is unchanged by its presence.
    """
    review = _make_review_critic(total_clears_threshold=False)  # sub-threshold
    comprehension = _make_comprehension_review()

    agg_review_only = aggregate([review])
    agg_with_comprehension = aggregate([review, comprehension])

    assert agg_review_only.verdict == Verdict.REVISE
    assert agg_with_comprehension.verdict == Verdict.REVISE
    assert agg_with_comprehension.total == agg_review_only.total


def test_comprehension_findings_union_into_aggregate(tmp_path):
    """The comprehension critic's major findings surface in the aggregate.

    memo-revise consumes the aggregated / per-sibling findings; this asserts
    the comprehension critic's findings are carried through the union (they
    are the whole point of the critic — the reviser addresses them).
    """
    review = _make_review_critic(total_clears_threshold=True)
    comprehension = _make_comprehension_review(with_findings=True)

    agg = aggregate([review, comprehension])

    # The comprehension critic's major findings are in the union.
    rationales = " ".join(f.rationale for f in agg.findings)
    assert "MISSING" in rationales
    assert "f_token" in rationales
    # Both critics' ids surface in the audit trail.
    assert sorted(agg.critic_ids) == ["comprehension", "review"]


def test_comprehension_review_round_trips_through_json(tmp_path):
    """Schema round-trip: build, write, discover, load, re-validate."""
    original = _make_comprehension_review()
    sidecar = tmp_path / "investment-memo.5.comprehension"
    _write_sidecar(sidecar, original)

    siblings = discover_critics(tmp_path / "investment-memo.5")
    assert siblings == [sidecar]

    loaded = load_review(sidecar)
    assert loaded.critic_id == original.critic_id
    assert loaded.version_dir == original.version_dir
    assert loaded.critical_flags == []

    # Validate the raw JSON on disk parses cleanly via the schema directly.
    raw = json.loads((sidecar / CANONICAL_REVIEW_FILENAME).read_text())
    re_validated = Review.model_validate(raw)
    assert re_validated.critic_id == "comprehension"
    assert all(s.score is None for s in re_validated.scores)
