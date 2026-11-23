"""检查已合并的版本标签并续传未完成的发布。"""
import argparse
import os
from pathlib import Path
import subprocess

from release_version import package_version


ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "haydardsx/iquji-community-agent"


def run(*args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, capture_output=True, text=True, **kwargs).stdout.strip()


def checked_tag():
    head = run("git", "rev-parse", "HEAD")
    tag = "v" + package_version(ROOT)
    run("git", "merge-base", "--is-ancestor", head, "refs/remotes/origin/main")
    return head, tag


def verify():
    head, tag = checked_tag()
    if os.environ.get("GITHUB_REF") != "refs/tags/" + tag or run("git", "rev-parse", tag + "^{commit}") != head:
        raise ValueError("标签、版本和已合并提交必须一致。")


def published_tags():
    return run("gh", "api", "--paginate", f"repos/{REPOSITORY}/releases?per_page=100",
               "--jq", ".[] | select(.draft == false and .prerelease == false) | .tag_name").splitlines()


def plan():
    if os.environ.get("GITHUB_REF") != "refs/heads/main":
        raise ValueError("发布入口需要 main。")
    head, tag = checked_tag()
    if not run("git", "tag", "--list", tag):
        return "create", tag
    tagged = run("git", "rev-parse", tag + "^{commit}")
    run("git", "merge-base", "--is-ancestor", tagged, head)
    return ("complete" if tag in published_tags() else "resume"), tag


def complete():
    verify()
    tag = "v" + package_version(ROOT)
    if tag in published_tags():
        return
    run("gh", "release", "create", tag, "--repo", REPOSITORY, "--verify-tag",
        "--title", tag, "--notes", "Published the agent library, CLI and wire protocol on crates.io.")


if __name__ == "__main__":
    actions = {"plan": plan, "verify": verify, "complete": complete}
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=actions)
    operation = parser.parse_args().operation
    if os.environ.get("GITHUB_REPOSITORY") != REPOSITORY:
        parser.error("发布仓库与包清单不一致。")
    if operation == "plan":
        action, tag = plan()
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            print(f"action={action}", f"tag={tag}", sep="\n", file=output)
    else:
        actions[operation]()
