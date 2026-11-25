"""以真实 Git 提交验证暂存快照、钩子顺序和失败结果。"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CI = """from pathlib import Path
import subprocess
import sys

if (Path('example.txt').read_text() != 'accepted\\n'
        or Path('untracked.txt').exists()
        or subprocess.check_output(['git', 'status', '--porcelain']).strip()):
    sys.exit('staged fixture rejected')
"""


class StagedCheckTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="staged-check-test-")
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)
        self.env = os.environ.copy()
        local_variables = subprocess.check_output(["git", "rev-parse", "--local-env-vars"], text=True)
        for name in local_variables.splitlines():
            self.env.pop(name, None)
        for role in ("AUTHOR", "COMMITTER"):
            for attribute in ("NAME", "EMAIL", "DATE"):
                self.env.pop(f"GIT_{role}_{attribute}", None)
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        self.git("init", "--quiet", "--template=", "--initial-branch=feat/staged-checks")
        self.git("config", "user.name", "IQuji Community")
        self.git("config", "user.email", "community@iqu.lol")
        for name in ("scripts/check_staged.py", ".github/scripts/check_subjects.py", ".github/scripts/community_guard.py"):
            destination = self.repo / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)
        self.write("scripts/check.py", CI)
        self.write("example.txt", "original\n")
        self.write("other.txt", "original\n")
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "test(test): prepare staged checks")
        self.run_command(sys.executable, "scripts/check_staged.py", "--install")

    def run_command(self, *command, check=True, env=None):
        return subprocess.run(command, cwd=self.repo, env=env or self.env,
                              capture_output=True, text=True, check=check)

    def git(self, *args, check=True):
        return self.run_command("git", *args, check=check)

    def write(self, name, contents):
        (self.repo / name).write_text(contents)

    def stage(self, content):
        self.write("example.txt", content)
        self.git("add", "example.txt")

    def commit_objects(self):
        return self.git("cat-file", "--batch-all-objects", "--batch-check=%(objecttype)").stdout.splitlines().count("commit")

    def test_uses_partially_staged_file_and_ignores_worktree_scripts(self):
        self.stage("accepted\n")
        self.write("example.txt", "accepted\nunstaged line\n")
        self.write("scripts/check.py", "raise SystemExit(1)\n")
        self.write("untracked.txt", "ignored fixture\n")
        self.git("commit", "--quiet", "-m", "test(test): record only staged content")
        self.assertEqual(self.git("show", "HEAD:example.txt").stdout, "accepted\n")
        self.assertEqual((self.repo / "example.txt").read_text(), "accepted\nunstaged line\n")
        self.assertEqual((self.repo / "scripts/check.py").read_text(), "raise SystemExit(1)\n")

    def test_failed_check_preserves_index_and_creates_no_candidate_commit(self):
        self.stage("blocked\n")
        self.write("example.txt", "accepted\n")
        head, tree, count = self.git("rev-parse", "HEAD").stdout, self.git("write-tree").stdout, self.commit_objects()
        result = self.git("commit", "-m", "test(test): refuse a failed check", check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("staged fixture rejected", result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD").stdout, head)
        self.assertEqual(self.git("write-tree").stdout, tree)
        self.assertEqual(self.commit_objects(), count)

    def test_respects_the_alternate_index_used_by_git_commit_only(self):
        self.write("example.txt", "accepted\n")
        self.write("other.txt", "pending\n")
        self.git("add", "other.txt")
        self.git("commit", "--only", "-m", "test(test): check the selected path", "example.txt")
        self.assertEqual(self.git("show", "HEAD:example.txt").stdout, "accepted\n")
        self.assertEqual(self.git("show", "HEAD:other.txt").stdout, "original\n")
        self.assertEqual(self.git("show", ":other.txt").stdout, "pending\n")

    def test_commit_message_hook_checks_real_author_and_subject(self):
        self.stage("accepted\n")
        head = self.git("rev-parse", "HEAD").stdout
        invalid_author = self.git("commit", "--author", "Example Person <person@example.invalid>",
                                  "-m", "test(test): verify author identity", check=False)
        invalid_title = self.git("commit", "-m", "fix(ci): fix", check=False)
        self.assertNotEqual(invalid_author.returncode, 0)
        self.assertIn("maintenance identity", invalid_author.stderr)
        self.assertNotEqual(invalid_title.returncode, 0)
        self.assertIn("Conventional Commits", invalid_title.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD").stdout, head)

    def test_pull_request_checks_its_branch_and_each_commit_title(self):
        base = self.git("rev-parse", "HEAD").stdout.strip()
        self.stage("accepted\n")
        self.git("commit", "--quiet", "-m", "test(test): verify the pull request")
        head = self.git("rev-parse", "HEAD").stdout.strip()
        path = self.repo / "event.json"
        event = {"pull_request": {"base": {"sha": base}, "head": {"ref": "test/staged-checks", "sha": head},
                                 "title": "test(test): verify the pull request"}}
        environment = dict(self.env, GITHUB_EVENT_PATH=str(path))
        command = (sys.executable, ".github/scripts/check_subjects.py", "--pull-request")
        path.write_text(json.dumps(event))
        self.run_command(*command, env=environment)
        for invalid_branch in ("reviewed/abcdef", "feat/uppercase-Slug", "test/" + "s" * 60):
            event["pull_request"]["head"]["ref"] = invalid_branch
            path.write_text(json.dumps(event))
            self.assertNotEqual(self.run_command(*command, env=environment, check=False).returncode, 0)

    def test_installer_preserves_an_existing_hook(self):
        hook = self.repo / ".git/hooks/pre-commit"
        hook.write_text("#!/bin/sh\nexit 0\n")
        result = self.run_command(sys.executable, "scripts/check_staged.py", "--install", check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(hook.read_text(), "#!/bin/sh\nexit 0\n")


if __name__ == "__main__":
    unittest.main()
