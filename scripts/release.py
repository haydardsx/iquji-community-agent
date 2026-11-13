"""校验并按依赖顺序发布公开 crate，支持中断后继续。"""

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tomllib
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = [ROOT / "protocol", ROOT]


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def registry_version(name, version):
    request = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{name}/{version}",
        headers={"User-Agent": "iquji-community-agent-release"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)["version"]
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def main():
    dry_run = "--dry-run" in sys.argv
    run("git", "diff", "--exit-code")
    run("git", "diff", "--cached", "--exit-code")
    run("cargo", "package", "--workspace", "--locked")
    releases = []
    for directory in PACKAGES:
        package = tomllib.loads((directory / "Cargo.toml").read_text())["package"]
        name, version = package["name"], package["version"]
        tag = os.environ.get("GITHUB_REF_NAME")
        if os.environ.get("GITHUB_REF_TYPE") == "tag" and tag != f"v{version}":
            raise RuntimeError("标签必须与两个 crate 的版本一致。")
        artifact = ROOT / "target/package" / f"{name}-{version}.crate"
        with tarfile.open(artifact) as archive:
            for member in archive.getmembers():
                if not member.isfile():
                    continue
                relative = member.name.split("/", 1)[1]
                if not (relative.startswith("src/") or relative in {
                    "Cargo.toml", "Cargo.toml.orig", "Cargo.lock", "README.md",
                    "LICENSE", "rust-toolchain.toml", "config.example.json", ".cargo_vcs_info.json",
                }):
                    raise RuntimeError(f"制品包含非发布文件：{relative}")
                content = archive.extractfile(member).read().decode("utf-8", errors="replace")
                if re.search(r"/(?:Users|home)/[A-Za-z0-9_.-]+/|[A-Z]:\\Users\\|-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----", content):
                    raise RuntimeError(f"制品包含本机信息或私密内容：{relative}")
        if dry_run:
            print(f"本地制品检查通过，尚未发布：{name}@{version}")
            continue
        checksum = hashlib.sha256(artifact.read_bytes()).hexdigest()
        published = registry_version(name, version)
        if published:
            if published["checksum"] != checksum:
                raise RuntimeError(f"{name}@{version} 已存在但内容不同，请发布新版本。")
            print(f"已发布且内容一致：{name}@{version}")
        else:
            releases.append((directory, name, version))
    for directory, name, version in releases:
        run("cargo", "publish", "--locked", "--manifest-path", str(directory / "Cargo.toml"))


if __name__ == "__main__":
    main()
