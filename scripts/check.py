"""运行 Agent CI 的当前快照检查。"""

import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def check():
    commands = (
        (sys.executable, ".github/scripts/community_guard.py", "--repo", ".", "--tree", "HEAD", "--contents-only"),
        (sys.executable, "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"),
        ("cargo", "fmt", "--all", "--", "--check"),
        ("cargo", "fetch", "--locked", "--manifest-path", "protocol/tests/fixtures/host/Cargo.toml"),
        ("cargo", "test", "--workspace", "--locked", "--", "--test-threads=1"),
        ("cargo", "clippy", "--workspace", "--locked", "--all-targets", "--", "-D", "warnings"),
        (sys.executable, "scripts/release.py", "--dry-run"),
    )
    for command in commands:
        subprocess.run(command, cwd=ROOT, check=True)


if __name__ == "__main__":
    try:
        check()
    except (OSError, subprocess.SubprocessError):
        raise SystemExit("Source, test or package verification failed.") from None
