"""验证经合并源码对应的发布标签。"""
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import auto_release


class ReleaseTagTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.enterContext(patch.object(auto_release, "ROOT", self.directory))
        self.enterContext(patch.object(auto_release, "package_version", return_value="0.2.0"))
        self.enterContext(patch.dict(os.environ, {"GITHUB_REF": "refs/tags/v0.2.0"}))
        self.git("init", "--quiet", "-b", "main")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "chore: create fixture")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("tag", "v0.2.0")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.directory, check=True, capture_output=True)

    def test_merged_tag(self):
        auto_release.verify()

    def test_unmerged_commit(self):
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "test: create fixture")
        with self.assertRaises(subprocess.CalledProcessError):
            auto_release.verify()

    def test_wrong_tag(self):
        with patch.dict(os.environ, {"GITHUB_REF": "refs/tags/v0.2.1"}):
            with self.assertRaises(ValueError):
                auto_release.verify()

    def test_plan_requests_protected_tag(self):
        self.git("tag", "-d", "v0.2.0")
        with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/main"}), \
                patch.object(auto_release, "published_tags") as published:
            self.assertEqual(auto_release.plan(), ("create", "v0.2.0"))
            published.assert_not_called()
        self.assertEqual(self.git("tag", "--list").stdout, b"")

    def test_plan_resumes_existing_tag_until_release_is_complete(self):
        with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/main"}):
            with patch.object(auto_release, "published_tags", return_value=[]):
                self.assertEqual(auto_release.plan(), ("resume", "v0.2.0"))
            with patch.object(auto_release, "published_tags", return_value=["v0.2.0"]):
                self.assertEqual(auto_release.plan(), ("complete", "v0.2.0"))
