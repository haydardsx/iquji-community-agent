"""检查进入拉取请求的新增提交身份。"""
import json
import os
from pathlib import Path
import re
import subprocess

EMAIL = b"community@iqu.lol"


def check(base, head):
    if not all(isinstance(sha, str) and re.fullmatch(r"[a-f0-9]{40}", sha) for sha in (base, head)):
        raise ValueError("Invalid pull request revision.")
    # 基线前的提交与 GitHub 为合并生成的提交分别由历史检查和平台保护规则核对。
    records = subprocess.check_output([
        "git", "--no-replace-objects", "log", "-z", "--format=%ae%x00%ce", base + ".." + head, "--",
    ]).split(b"\0")
    if records == [b""] or records[-1] or len(records) % 2 != 1 or any(value != EMAIL for value in records[:-1]):
        raise ValueError("Use the project email for both Git identities.")


if __name__ == "__main__":
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_bytes())["pull_request"]
    try:
        check(event["base"]["sha"], event["head"]["sha"])
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit("Pull request commit identity verification failed.") from error
