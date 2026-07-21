from __future__ import annotations

import tempfile
import unittest
import zipfile
from pathlib import Path, PurePath

from tools import archive_go_27


class Go27ArchiveTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.repo = Path(__file__).resolve().parents[1]

    def test_committed_archive_matches_tag(self) -> None:
        directory = self.repo / "archive" / archive_go_27.TAG
        commit, count = archive_go_27.verify(
            self.repo,
            directory / archive_go_27.ARCHIVE_NAME,
            directory / "SHA256SUMS.txt",
        )
        self.assertEqual(commit, "a4bf37fcde40c5342de0b6b72348f32e876cb5ec")
        self.assertGreater(count, 90)

    def test_active_tree_has_no_go_build_inputs(self) -> None:
        tracked_and_untracked = archive_go_27.run_git(
            self.repo, "ls-files", "-co", "--exclude-standard", "-z"
        )
        paths = [
            path.decode("utf-8")
            for path in tracked_and_untracked.split(b"\0")
            if path
        ]
        go_inputs = [
            path
            for path in paths
            if path.endswith(".go") or PurePath(path).name in {"go.mod", "go.sum"}
        ]
        self.assertEqual(go_inputs, [])

    def test_build_is_reproducible(self) -> None:
        with tempfile.TemporaryDirectory(prefix="dh-go27-archive-") as temporary:
            root = Path(temporary)
            first = root / "first.zip"
            second = root / "second.zip"
            archive_go_27.build(self.repo, first, root / "first.sha256")
            archive_go_27.build(self.repo, second, root / "second.sha256")
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_verifier_rejects_extra_runtime_data(self) -> None:
        with tempfile.TemporaryDirectory(prefix="dh-go27-tamper-") as temporary:
            root = Path(temporary)
            output = root / archive_go_27.ARCHIVE_NAME
            sums = root / "SHA256SUMS.txt"
            archive_go_27.build(self.repo, output, sums)
            with zipfile.ZipFile(output, "a") as archive:
                archive.writestr(
                    f"{archive_go_27.PREFIX}/runtime/dh.db", b"test data"
                )
            sums.write_text(
                f"{archive_go_27.checksum(output)}  {output.name}\n", encoding="ascii"
            )
            with self.assertRaises(archive_go_27.ArchiveError):
                archive_go_27.verify(self.repo, output, sums)


if __name__ == "__main__":
    unittest.main()
