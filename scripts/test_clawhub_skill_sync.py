import tempfile
import unittest
from pathlib import Path

from clawhub_skill_sync import SyncError, derive_changelog


class ReleaseChangelogTests(unittest.TestCase):
    def derive(self, changelog: str) -> str:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[package]\nversion = "0.6.1"\n')
            (root / "CHANGELOG.md").write_text(changelog)
            return derive_changelog(root)

    def test_uses_promoted_notes_for_current_package_release(self):
        notes = self.derive(
            "## Unreleased\n\n## 0.6.1 - 2026-09-13\n### Changed\n"
            "- Publish clipboard-memory 1.3.9 with corrected health checks.\n"
            "## 0.6.0 - 2026-07-11\n- Old skill notes.\n"
        )
        self.assertIn("1.3.9", notes)
        self.assertNotIn("Old", notes)

    def test_unreleased_notes_take_precedence(self):
        self.assertEqual(
            self.derive(
                "## Unreleased\n- New skill change.\n\n## 0.6.1\n- Old skill change.\n"
            ),
            "New skill change",
        )

    def test_does_not_reuse_notes_from_a_different_release(self):
        with self.assertRaises(SyncError):
            self.derive("## Unreleased\n\n## 0.6.0\n- Old skill change.\n")


if __name__ == "__main__":
    unittest.main()
