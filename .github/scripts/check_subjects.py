"""校验用途分支、提交标题和拉取请求标题。"""

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True
from community_guard import check_text


TYPES = "feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert"
SCOPES = "agent|cli|wire|protocol|config|runtime|docs|deps|release|ci|test|build"
BRANCH = re.compile(rf"(?:{TYPES})/[a-z0-9]+(?:-[a-z0-9]+)*")
SUBJECT = re.compile(rf"(?:{TYPES})\((?:{SCOPES})\)!?: ([^\r\n]+)")
PLACEHOLDERS = {"update", "fix", "wip", "tmp", "test", "change", "updates", "changes", "todo", "修改", "更新", "测试", "修复"}


def check_branch(branch):
    if len(branch) > 64 or not BRANCH.fullmatch(branch):
        raise ValueError("Use a development branch such as feat/agent-discovery.")


def check_subject(title):
    match = SUBJECT.fullmatch(title)
    if title.isprintable() and len(title) <= 100 and match:
        summary = match[1]
        words = "".join(char for char in summary.casefold() if char.isalnum())
        if (summary == summary.strip() and sum(char.isalnum() for char in summary) >= 2
                and words not in PLACEHOLDERS):
            return
    raise ValueError("Use a Conventional Commits title with a descriptive summary and a supported scope.")


def git(*args):
    return subprocess.check_output(["git", "--no-replace-objects", *args], text=True)


def check_pull_request(event):
    pull = event["pull_request"]
    check_branch(pull["head"]["ref"])
    check_subject(pull["title"])
    base, head = pull["base"]["sha"], pull["head"]["sha"]
    if not all(re.fullmatch(r"[a-f0-9]{40}", revision) for revision in (base, head)):
        raise ValueError("Invalid pull request revision.")
    subjects = git("log", "-z", "--format=%s", f"{base}..{head}", "--")
    if not subjects:
        raise ValueError("Pull request has no new commits.")
    for subject in subjects.split("\0")[:-1]:
        check_subject(subject)


def check_commit_message(message):
    # 与 Git 自身使用同一套清理规则，不把模板注释当作标题。
    subject, *_ = subprocess.run(
        ["git", "stripspace", "--strip-comments"], input=message, text=True,
        stdout=subprocess.PIPE, check=True,
    ).stdout.split("\n", 1)
    check_subject(subject)
    if check_text("commit", message):
        raise ValueError("Commit message failed the public source check.")
    for role in ("GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"):
        if not re.fullmatch(r"IQuji Community <community@iqu\.lol> \d+ [+-]\d{4}\n", git("var", role)):
            raise ValueError("Use the project Git maintenance identity for the author and committer.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--pull-request", action="store_true")
    mode.add_argument("--commit-message", type=Path)
    options = parser.parse_args()
    try:
        if options.pull_request:
            check_pull_request(json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()))
        else:
            check_commit_message(options.commit_message.read_text())
    except (OSError, KeyError, ValueError, subprocess.SubprocessError) as error:
        raise SystemExit(str(error) if type(error) is ValueError else "Submission verification failed.") from None
