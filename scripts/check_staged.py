"""在隔离检出检查暂存快照，或安装本仓提交检查。"""

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / ".github/scripts"))
from check_subjects import check_branch


def git(*args, cwd=ROOT, env=None):
    return subprocess.check_output(
        ["git", "--no-replace-objects", *args], cwd=cwd, env=env, text=True,
    ).strip()


def check_staged():
    branch = git("symbolic-ref", "--short", "HEAD")
    check_branch(branch)
    head = git("rev-parse", "HEAD")
    tree = git("write-tree")
    environment = os.environ.copy()
    for name in git("rev-parse", "--local-env-vars").splitlines():
        environment.pop(name, None)

    with tempfile.TemporaryDirectory(prefix="community-staged-") as directory:
        snapshot = Path(directory)
        git("clone", "--quiet", "--shared", "--no-checkout", "--template=", "--", str(ROOT), directory, env=environment)
        git("read-tree", tree, cwd=snapshot, env=environment)
        git("checkout-index", "--all", "--force", cwd=snapshot, env=environment)
        # 制品检查需要干净检出。临时提交只在隔离仓内存在。
        commit_environment = {
            **environment,
            "GIT_AUTHOR_NAME": "IQuji Community", "GIT_AUTHOR_EMAIL": "community@iqu.lol",
            "GIT_COMMITTER_NAME": "IQuji Community", "GIT_COMMITTER_EMAIL": "community@iqu.lol",
        }
        commit = git(
            "-c", "commit.gpgsign=false", "commit-tree", tree, "-p", head,
            "-m", "ci(ci): check staged snapshot", cwd=snapshot, env=commit_environment,
        )
        git("update-ref", "HEAD", commit, cwd=snapshot, env=environment)
        subprocess.run([sys.executable, "scripts/check.py"], cwd=snapshot, env=environment, check=True)

    if (git("write-tree"), git("rev-parse", "HEAD"), git("symbolic-ref", "--short", "HEAD")) != (tree, head, branch):
        raise ValueError("暂存区或当前分支在检查期间变化，请重新运行提交检查。")


def install():
    configured = subprocess.run(
        ["git", "config", "--get", "core.hooksPath"], cwd=ROOT,
        stdout=subprocess.DEVNULL, check=False,
    )
    if configured.returncode != 1:
        raise ValueError("Git 已设置 core.hooksPath，请保留现有设置并将检查命令接入现有钩子。")
    hooks = {
        "pre-commit": "#!/bin/sh\nexec python3 scripts/check_staged.py\n",
        "commit-msg": '#!/bin/sh\nexec python3 .github/scripts/check_subjects.py --commit-message "$1"\n',
    }
    paths = {name: ROOT / git("rev-parse", "--git-path", "hooks/" + name) for name in hooks}
    for name, path in paths.items():
        if path.is_symlink() or (path.exists() and (not path.is_file() or path.read_text() != hooks[name])):
            raise ValueError(f"已有 {name} 钩子，请保留现有钩子并接入检查命令。")
    for name, path in paths.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            with path.open("x") as file:
                file.write(hooks[name])
        path.chmod(0o755)
    print("已安装暂存快照和提交标题检查。")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--install", action="store_true", help="安装 Git 提交钩子")
    arguments = parser.parse_args()
    try:
        (install if arguments.install else check_staged)()
    except ValueError as error:
        raise SystemExit(str(error)) from None
    except (OSError, subprocess.SubprocessError):
        raise SystemExit("提交前检查失败。") from None
