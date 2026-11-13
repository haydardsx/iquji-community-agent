"""版本快照、重复源码与中断续传的发布决策回归。"""

from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import auto_release
from release_version import (
    AutomaticTag, VERSION_FILES, choose_release, next_version,
    package_version, rewrite_versions, stable,
)


SOURCE_ROOT = Path(__file__).resolve().parents[1]


class VersionTests(unittest.TestCase):
    def test_next_patch_uses_all_sources_and_numeric_order(self):
        self.assertEqual(next_version("0.1.0", ["0.1.9", "0.1.10", "0.2.0-rc.1"]), "0.1.11")
        self.assertEqual(next_version("1.0.0", ["0.9.99"]), "1.0.1")
        self.assertEqual(next_version("0.1.0", ["1.4.2", "1.4.3"]), "1.4.4")

    def test_only_canonical_stable_versions_count(self):
        for value in ("01.2.3", "1.02.3", "1.2.3-beta", "1.2.3+build", "v1.2.3", "1.2"):
            self.assertIsNone(stable(value))
        with self.assertRaises(ValueError):
            next_version("0.1.0-dev", [])

    def test_rewrite_updates_only_local_versions_and_internal_dependency(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            originals = {}
            for name in VERSION_FILES:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                originals[name] = (SOURCE_ROOT / name).read_text()
                path.write_text(originals[name])
            old = package_version(root)
            rewrite_versions(root, "9.8.7")
            self.assertEqual(package_version(root), "9.8.7")
            rewrite_versions(root, old)
            self.assertEqual({name: (root / name).read_text() for name in VERSION_FILES}, originals)

    def test_invalid_lock_does_not_partially_modify_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in VERSION_FILES:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(SOURCE_ROOT / name, path)
            path = root / VERSION_FILES[-1]
            path.write_text(path.read_text().replace('name = "iquji-agent-wire"', 'name = "other-wire"'))
            before = {name: (root / name).read_text() for name in VERSION_FILES}
            with self.assertRaises(ValueError):
                rewrite_versions(root, "9.8.7")
            self.assertEqual({name: (root / name).read_text() for name in VERSION_FILES}, before)


class DecisionTests(unittest.TestCase):
    def test_completed_source_does_not_allocate_again(self):
        tag = AutomaticTag("v0.1.1", "source-a", "snapshot-a")
        self.assertEqual(choose_release("source-a", [tag], {tag.name}), ("done", None))
        self.assertEqual(choose_release("latest-source", [tag], {tag.name}), ("create", None))

    def test_partial_version_precedes_new_main(self):
        partial = AutomaticTag("v0.1.2", "old-source", "snapshot-a")
        later = AutomaticTag("v0.1.10", "other-source", "snapshot-b")
        self.assertEqual(choose_release("latest-source", [later, partial], set()), ("retry", partial))
        self.assertEqual(choose_release("latest-source", [later, partial], {partial.name}), ("retry", later))

    def test_duplicate_source_is_rejected(self):
        tags = [AutomaticTag(f"v0.1.{number}", "same-source", "snapshot") for number in (1, 2)]
        with self.assertRaises(ValueError):
            choose_release("same-source", tags, set())

    def test_empty_history_only_plans_current_source(self):
        self.assertEqual(choose_release("latest-source", [], set()), ("create", None))

    def test_snapshot_validation_does_not_inherit_publication_credentials(self):
        with patch.dict(os.environ, {"GH_TOKEN": "test", "GITHUB_TOKEN": "test", "CARGO_REGISTRY_TOKEN": "test",
                                    "ACTIONS_ID_TOKEN_REQUEST_URL": "test", "ACTIONS_RUNTIME_TOKEN": "test"}):
            with patch.object(auto_release, "run") as execute:
                auto_release.validate_snapshot(Path("snapshot"))
        self.assertEqual(execute.call_count, 6)
        for call in execute.call_args_list:
            self.assertFalse(call.kwargs["inherit_env"])
            self.assertFalse(any(name.endswith("_TOKEN") or name.startswith("ACTIONS_ID_TOKEN_")
                                 for name in call.kwargs["env"]))

    def test_dispatch_run_requires_exact_tag_commit_and_event(self):
        tag = AutomaticTag("v0.1.1", "source", "snapshot")
        exact = {"id": 1, "head_sha": "snapshot", "head_branch": "v0.1.1", "event": "workflow_dispatch"}
        runs = [exact, {**exact, "head_sha": "other"}, {**exact, "head_branch": "main"}, {**exact, "event": "push"}]
        with patch.object(auto_release, "api", return_value={"workflow_runs": runs}):
            self.assertEqual(auto_release.workflow_runs(tag), [exact])

    def test_completed_draft_preserves_newer_latest_release(self):
        releases = [{"tag_name": "v0.1.1", "draft": True, "prerelease": False},
                    {"tag_name": "v0.1.2", "draft": False, "prerelease": False}]
        with patch.object(auto_release, "package_version", return_value="0.1.1"):
            with patch.object(auto_release, "pages", return_value=releases), patch.object(auto_release, "run") as execute:
                with patch.dict(os.environ, {"GITHUB_REF": "refs/tags/v0.1.1"}):
                    auto_release.complete()
        arguments = execute.call_args.args
        self.assertEqual(arguments[:4], ("gh", "release", "edit", "v0.1.1"))
        self.assertIn("--draft=false", arguments)
        self.assertIn("--latest=false", arguments)

    def test_existing_completion_is_idempotent(self):
        with patch.object(auto_release, "package_version", return_value="0.1.1"):
            with patch.object(auto_release, "pages", return_value=[{"tag_name": "v0.1.1", "draft": False, "prerelease": False}]):
                with patch.object(auto_release, "run") as execute, patch.dict(os.environ, {"GITHUB_REF": "refs/tags/v0.1.1"}):
                    auto_release.complete()
        execute.assert_not_called()


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / "repository"
        self.root.mkdir()
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Release test")
        self.git("config", "user.email", "release-test@example.invalid")
        for name in VERSION_FILES:
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(SOURCE_ROOT / name, target)
        self.git("add", ".")
        self.git("commit", "-m", "开发基线")
        self.source = self.git("rev-parse", "HEAD")
        self.git("remote", "add", "origin", str(self.root))
        self.patcher = patch.object(auto_release, "ROOT", self.root)
        self.patcher.start()

    def tearDown(self):
        self.patcher.stop()
        self.temporary.cleanup()

    def git(self, *arguments):
        return subprocess.check_output(["git", *arguments], cwd=self.root, text=True, stderr=subprocess.DEVNULL).strip()

    def test_snapshot_has_main_parent_bot_identity_and_version_only_tree(self):
        with patch.object(auto_release, "validate_snapshot") as validate:
            tag = auto_release.create_snapshot(self.source, "9.8.7")
        validate.assert_called_once()
        self.assertEqual(self.git("rev-parse", "main"), self.source)
        self.assertEqual(self.git("show", "-s", "--format=%P", tag.commit), self.source)
        self.assertEqual(self.git("show", "-s", "--format=%ae", tag.commit), auto_release.BOT["GIT_AUTHOR_EMAIL"])
        self.assertEqual(set(self.git("diff", "--name-only", self.source, tag.commit).splitlines()), set(VERSION_FILES))
        self.assertIn(f"Auto-Release-Source: {self.source}", self.git("for-each-ref", "--format=%(contents)", "refs/tags/v9.8.7"))
        self.assertEqual(auto_release.inspect_tags()[1], [tag])

    def test_failed_validation_does_not_create_tag(self):
        with patch.object(auto_release, "validate_snapshot", side_effect=RuntimeError("检查未通过")):
            with self.assertRaises(RuntimeError):
                auto_release.create_snapshot(self.source, "9.8.7")
        self.assertEqual(self.git("tag", "--list"), "")
        self.assertEqual(self.git("rev-parse", "main"), self.source)

    def test_new_main_during_validation_discards_old_candidate(self):
        def advance(_directory):
            self.git("commit", "--allow-empty", "-m", "新的 main")
        with patch.object(auto_release, "validate_snapshot", side_effect=advance):
            self.assertIsNone(auto_release.create_snapshot(self.source, "9.8.7"))
        self.assertEqual(self.git("tag", "--list"), "")
        self.assertNotEqual(self.git("rev-parse", "main"), self.source)

    def test_malformed_automatic_marker_is_not_treated_as_manual_tag(self):
        self.git("tag", "-a", "v0.1.1", "-m", "Auto-Release-Source: malformed")
        with self.assertRaisesRegex(RuntimeError, "格式错误"):
            auto_release.inspect_tags()


if __name__ == "__main__":
    unittest.main()
