"""串行发布最新 main 快照；版本提交和自动标签均不回写 main。"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import tomllib
import urllib.error
import urllib.request

from release_version import (
    AutomaticTag, CRATES, VERSION_FILES, choose_release, next_version,
    package_version, rewrite_versions, stable,
)


ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "haydardsx/iquji-community-agent"
BOT = {"GIT_AUTHOR_NAME": "github-actions[bot]", "GIT_COMMITTER_NAME": "github-actions[bot]",
       "GIT_AUTHOR_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
       "GIT_COMMITTER_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com"}


def run(*args, cwd=None, capture=False, env=None, inherit_env=True):
    result = subprocess.run(args, cwd=cwd or ROOT, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None,
                            env={**(os.environ if inherit_env else {}), **(env or {})})
    return result.stdout.strip() if capture else None


def git(*args, **kwargs):
    return run("git", *args, **kwargs)


def api(path):
    return json.loads(run("gh", "api", f"repos/{REPOSITORY}/{path}".rstrip("/"), capture=True))


def pages(path):
    result = []
    for page in range(1, 1001):
        entries = api(f"{path}?per_page=100&page={page}")
        result.extend(entries)
        if len(entries) < 100:
            return result
    raise RuntimeError("GitHub 分页超过预期，拒绝使用不完整发布历史。")


def releases():
    return [item for item in pages("releases") if not item["draft"] and not item["prerelease"]]


def registry_versions(name):
    request = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{name}",
        headers={"User-Agent": "iquji-community-agent-auto-release"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return [item["num"] for item in json.load(response)["versions"]]
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return []
        raise


def inspect_tags():
    names = git("tag", "--list", "v*", capture=True).splitlines()
    stable_tags = [name for name in names if stable(name[1:]) is not None]
    automatic = []
    for name in stable_tags:
        reference = f"refs/tags/{name}"
        if git("cat-file", "-t", reference, capture=True) != "tag":
            continue
        message = git("for-each-ref", "--format=%(contents)", reference, capture=True)
        sources = re.findall(r"^Auto-Release-Source: ([0-9a-f]{40})$", message, re.MULTILINE)
        if "Auto-Release-Source" not in message:
            continue
        if len(sources) != 1 or message.count("Auto-Release-Source") != 1:
            raise RuntimeError(f"自动标签的源码身份缺失、格式错误或重复：{name}")
        commit = git("rev-parse", f"{reference}^{{commit}}", capture=True)
        if git("show", "-s", "--format=%P", commit, capture=True) != sources[0]:
            raise RuntimeError(f"自动标签必须直接继承其 main 源码：{name}")
        for manifest, expected in zip(VERSION_FILES[:2], reversed(CRATES)):
            package = tomllib.loads(git("show", f"{commit}:{manifest}", capture=True))["package"]
            if package["version"] != name[1:] or package["name"] != expected:
                raise RuntimeError(f"自动标签版本或包身份与源码不一致：{name}")
        automatic.append(AutomaticTag(name, sources[0], commit))
    return stable_tags, automatic


def validate_snapshot(directory):
    environment = {key: value for key, value in os.environ.items()
                   if not key.endswith(("_TOKEN", "_PASSWORD")) and not key.startswith("ACTIONS_ID_TOKEN_")}
    commands = [
        ("python3", "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"),
        ("cargo", "fmt", "--all", "--", "--check"),
        ("cargo", "fetch", "--locked", "--manifest-path", "protocol/tests/fixtures/host/Cargo.toml"),
        ("cargo", "test", "--workspace", "--locked", "--", "--test-threads=1"),
        ("cargo", "clippy", "--workspace", "--locked", "--all-targets", "--", "-D", "warnings"),
        ("python3", "scripts/release.py", "--dry-run"),
    ]
    for command in commands:
        run(*command, cwd=directory, env=environment, inherit_env=False)


def create_snapshot(source, version):
    with tempfile.TemporaryDirectory(prefix="agent-release-") as temporary:
        directory = Path(temporary) / "snapshot"
        git("worktree", "add", "--detach", str(directory), source)
        try:
            rewrite_versions(directory, version)
            git("add", "--", *VERSION_FILES, cwd=directory)
            git("-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "commit", "-m",
                f"发布版本快照 v{version}", cwd=directory, env=BOT)
            commit = git("rev-parse", "HEAD", cwd=directory, capture=True)
            validate_snapshot(directory)
            git("diff", "--exit-code", cwd=directory)
            git("diff", "--cached", "--exit-code", cwd=directory)
            git("fetch", "origin", "+refs/heads/main:refs/remotes/origin/main")
            if git("rev-parse", "refs/remotes/origin/main", capture=True) != source:
                print("验证期间 main 已更新，重新规划最新源码。", flush=True)
                return None
            tag = f"v{version}"
            git("tag", "-a", tag, commit, "-m",
                f"自动发布 {tag}\n\nAuto-Release-Source: {source}", env=BOT)
            return AutomaticTag(tag, source, commit)
        finally:
            git("worktree", "remove", "--force", str(directory))


def workflow_runs(tag):
    response = api(f"actions/workflows/release.yml/runs?event=workflow_dispatch&branch={tag.name}&per_page=100")
    return [item for item in response["workflow_runs"]
            if item["head_sha"] == tag.commit and item["head_branch"] == tag.name
            and item["event"] == "workflow_dispatch"]


def publish_and_wait(tag):
    existing = workflow_runs(tag)
    active = [item for item in existing if item["status"] != "completed"]
    if active:
        selected = max(active, key=lambda item: item["id"])
    else:
        previous = {item["id"] for item in existing}
        run("gh", "workflow", "run", "release.yml", "--repo", REPOSITORY,
            "--ref", tag.name, "-f", "operation=publish")
        deadline = time.monotonic() + 180
        while True:
            candidates = [item for item in workflow_runs(tag) if item["id"] not in previous]
            if candidates:
                selected = max(candidates, key=lambda item: item["id"])
                break
            if time.monotonic() >= deadline:
                raise RuntimeError(f"尚未发现 {tag.name} 的发布工作流，后续重跑将继续此标签。")
            time.sleep(10)
    deadline = time.monotonic() + 90 * 60
    previous_status = None
    while True:
        state = api(f"actions/runs/{selected['id']}")
        if state["status"] != previous_status:
            print(f"{tag.name} publisher {selected['id']}: {state['status']}", flush=True)
            previous_status = state["status"]
        if state["status"] == "completed":
            if state["conclusion"] != "success":
                raise RuntimeError(f"{tag.name} 发布未完成：{state['conclusion']}；保留标签供重跑。")
            break
        if time.monotonic() >= deadline:
            raise RuntimeError(f"等待 {tag.name} 发布超时；保留标签供重跑。")
        time.sleep(15)
    if tag.name not in {item["tag_name"] for item in releases()}:
        raise RuntimeError(f"{tag.name} 缺少成功发布标记，不能继续下一个版本。")


def plan():
    if os.environ.get("GITHUB_REF") != "refs/heads/main":
        raise RuntimeError("自动发布规划只能从 main 运行。")
    if api("")["private"]:
        raise RuntimeError("仅允许从公开仓库发布。")
    expected_remote = f"https://github.com/{REPOSITORY}"
    if git("remote", "get-url", "origin", capture=True).removesuffix(".git") != expected_remote:
        raise RuntimeError("origin 必须是正式公开仓库的 HTTPS 地址。")
    while True:
        git("fetch", "origin", "+refs/heads/main:refs/remotes/origin/main", "--tags")
        source = git("rev-parse", "refs/remotes/origin/main", capture=True)
        tags, automatic = inspect_tags()
        action, retry = choose_release(source, automatic, {item["tag_name"] for item in releases()})
        if action == "done":
            print("最新 main 已完成发布，无需分配新版本。")
            return
        if action == "retry":
            publish_and_wait(retry)
            continue
        baseline = tomllib.loads(git("show", f"{source}:Cargo.toml", capture=True))["package"]["version"]
        observed = [name[1:] for name in tags]
        for name in CRATES:
            observed.extend(registry_versions(name))
        version = next_version(baseline, observed)
        tag = create_snapshot(source, version)
        if tag is None:
            continue
        # 临时凭据只由 gh 在 Git 需要认证时提供，不写入远端 URL 或提交。
        git("-c", "credential.helper=", "-c", "credential.helper=!gh auth git-credential",
            "push", "origin", f"refs/tags/{tag.name}")
        publish_and_wait(tag)


def complete():
    version = package_version(ROOT)
    tag = f"v{version}"
    if os.environ.get("GITHUB_REF") != f"refs/tags/{tag}":
        raise RuntimeError("发布完成标记只能由对应版本标签创建。")
    all_releases = pages("releases")
    existing = [item for item in all_releases if not item["draft"] and not item["prerelease"]]
    if tag in {item["tag_name"] for item in existing}:
        print(f"{tag} 已有发布完成标记。")
        return
    versions = [stable(item["tag_name"][1:]) for item in existing if item["tag_name"].startswith("v")]
    latest = max([item for item in versions if item is not None], default=(0, 0, 0))
    command = ["gh", "release", "create", tag, "--repo", REPOSITORY, "--verify-tag", "--title", tag,
               "--notes", "两个 crate 已通过检查并发布至 crates.io：iquji-agent-wire、iquji-shared-agent。"]
    if any(item["tag_name"] == tag for item in all_releases):
        command = ["gh", "release", "edit", tag, "--repo", REPOSITORY, "--draft=false", "--prerelease=false"]
    run(*command, "--latest" if stable(version) >= latest else "--latest=false")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("plan", "complete"))
    options = parser.parse_args()
    if os.environ.get("GITHUB_REPOSITORY") != REPOSITORY:
        raise RuntimeError("发布入口只能用于正式 Community Agent 仓库。")
    if options.operation == "plan":
        plan()
    else:
        complete()


if __name__ == "__main__":
    main()
