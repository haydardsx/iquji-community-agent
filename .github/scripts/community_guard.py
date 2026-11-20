"""检查版本控制中的文件及发布提交，只报告位置摘要和检查类别。"""
import argparse
import hashlib
from pathlib import PurePosixPath
import re
import subprocess
import sys


EXPECTED_ROOT = {
    ".gitignore", ".repository.json", "Cargo.lock", "Cargo.toml", "LICENSE",
    "README.md", "README.zh-CN.md", "config.example.json", "rust-toolchain.toml",
}
TEXT_TYPES = {".rs", ".json", ".toml", ".py", ".yml", ".md", ".lock"}
RULES = {
    "private-key": re.compile(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----"),
    "credential": re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|npm_[A-Za-z0-9]{30,}|AKIA[0-9A-Z]{16})"),
    "home-directory": re.compile(r"(?:/(?:Users|home)/[\w.-]+/|[A-Z]:\\Users\\[\w.-]+\\)"),
}


def git(repo, *args):
    return subprocess.check_output(["git", "-C", repo, *args], stderr=subprocess.DEVNULL)


def check_text(identity, contents):
    return [(identity, name) for name, expression in RULES.items() if expression.search(contents)]


def allowed_name(path):
    if len(path.parts) == 1:
        return path.name in EXPECTED_ROOT
    if path.parts[0] not in {"src", "tests", "protocol", ".github", "scripts"}:
        return False
    if path.suffix == ".md":
        return path.name in {"README.md", "README.zh-CN.md"}
    return path.suffix in TEXT_TYPES or path.name in {"LICENSE", ".gitignore"}


def check(repo, head, history, contents_only=False):
    findings = []
    seen = set()
    revisions = git(repo, "rev-list", head).splitlines() if history else [head.encode()]
    for raw_revision in revisions:
        revision = raw_revision.decode()
        if not contents_only:
            author, committer, message = git(repo, "show", "-s", "--format=%ae%x00%ce%x00%B", revision).decode().split("\0", 2)
            if not author.endswith("@users.noreply.github.com") or not (
                committer.endswith("@users.noreply.github.com") or committer == "noreply@github.com"
            ):
                findings.append((revision[:12], "commit-identity"))
            findings.extend(check_text(revision[:12], message))
        for entry in git(repo, "ls-tree", "-rz", revision).split(b"\0"):
            if not entry:
                continue
            metadata, filename = entry.split(b"\t", 1)
            mode, kind, oid = metadata.split()
            if (filename, oid) in seen:
                continue
            seen.add((filename, oid))
            name = filename.decode()
            public_id = hashlib.sha256(filename).hexdigest()[:12]
            if mode not in (b"100644", b"100755") or kind != b"blob" or not allowed_name(PurePosixPath(name)):
                findings.append((public_id, "file-type"))
                continue
            body = git(repo, "cat-file", "blob", oid.decode()).decode()
            if "\0" in body:
                findings.append((public_id, "binary"))
            findings.extend(check_text(public_id, body))
    return findings


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=".")
    parser.add_argument("--tree", default="HEAD")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--history", action="store_true")
    mode.add_argument("--contents-only", action="store_true")
    args = parser.parse_args()
    try:
        commit = git(args.repo, "rev-parse", "--verify", "--end-of-options", args.tree + "^{commit}").decode().strip()
        findings = check(args.repo, commit, args.history, args.contents_only)
    except (OSError, ValueError, subprocess.SubprocessError):
        sys.exit("Source verification failed.")
    for identity, category in findings:
        print(identity, category)
    sys.exit(bool(findings))
