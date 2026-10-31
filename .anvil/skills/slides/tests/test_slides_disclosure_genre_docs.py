"""Doc-contract tests for slides disclosure-vs-provenance and ``talk_genre`` (issue #1370).

Guards that the rubric and critic/reviser command docs carry the
"On-slide disclosure vs. provenance" guidance and the optional ``talk_genre``
BRIEF key (default ``technical-lecture``), and that the /44 weights are unchanged.

Slides-distinct filename per the #58 packaging convention.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

SKILL = Path(__file__).resolve().parents[1]
RUBRIC = SKILL / "rubric.md"
CMD = SKILL / "commands"


def _read(p: Path) -> str:
    return p.read_text(encoding="utf-8")


class DisclosureGenreDocsTests(unittest.TestCase):
    def test_rubric_has_disclosure_section(self) -> None:
        text = _read(RUBRIC)
        self.assertIn("On-slide disclosure vs. provenance", text)
        self.assertIn("audience common-sense test", text)

    def test_rubric_documents_talk_genre_and_default(self) -> None:
        text = _read(RUBRIC)
        self.assertIn("talk_genre", text)
        self.assertIn("maker-story", text)
        self.assertRegex(text, r"`technical-lecture` \(the default")

    def test_weights_still_sum_to_44(self) -> None:
        rows = re.findall(r"^\| (\d) \| \*\*[^|]+\*\* \| (\d+) \|", _read(RUBRIC), re.M)
        self.assertEqual(len(rows), 9)
        self.assertEqual(sum(int(w) for _, w in rows), 44)

    def test_critic_commands_reference_section(self) -> None:
        for name in ("slides-review", "slides-vision", "slides-audit", "slides-revise"):
            text = _read(CMD / f"{name}.md")
            self.assertIn("On-slide disclosure vs. provenance", text, name)

    def test_review_and_vision_honor_talk_genre(self) -> None:
        for name in ("slides-review", "slides-vision"):
            self.assertIn("talk_genre", _read(CMD / f"{name}.md"), name)

    def test_outline_and_example_brief_document_key(self) -> None:
        outline = _read(CMD / "slides-outline.md")
        self.assertIn("`talk_genre`", outline)
        self.assertIn("technical-lecture", outline)
        self.assertIn("talk_genre:", _read(SKILL / "assets" / "example-brief.md"))


if __name__ == "__main__":
    unittest.main()
