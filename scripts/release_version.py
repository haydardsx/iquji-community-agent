"""自动发布的稳定版本、重试决策与最小版本改写。"""

from dataclasses import dataclass
from pathlib import Path
import re
import tomllib


STABLE = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z")
VERSION_FILES = (
    "Cargo.toml", "protocol/Cargo.toml", "Cargo.lock",
    "protocol/tests/fixtures/host/Cargo.lock",
)
CRATES = ("iquji-agent-wire", "iquji-shared-agent")


def stable(value):
    match = STABLE.fullmatch(value)
    return tuple(map(int, match.groups())) if match else None


def next_version(baseline, observed):
    current = stable(baseline)
    if current is None:
        raise ValueError("源码基线必须是稳定版本。")
    latest = max([current, *(value for item in observed if (value := stable(item)))])
    return f"{latest[0]}.{latest[1]}.{latest[2] + 1}"


@dataclass(frozen=True)
class AutomaticTag:
    name: str
    source: str
    commit: str


def choose_release(source, automatic_tags, completed):
    """先续传最早未完成版本；同一源码有完成标记后不再分配版本。"""
    sources = [tag.source for tag in automatic_tags]
    if len(sources) != len(set(sources)):
        raise ValueError("同一源码存在多个自动版本，需先核验标签。")
    pending = [tag for tag in automatic_tags if tag.name not in completed]
    if pending:
        return "retry", min(pending, key=lambda tag: stable(tag.name[1:]))
    if source in sources:
        return "done", None
    return "create", None


def package_version(root):
    manifests = [tomllib.loads((root / name).read_text()) for name in VERSION_FILES[:2]]
    agent, wire = manifests
    version = agent["package"]["version"]
    if (stable(version) is None or wire["package"]["version"] != version
            or agent["dependencies"]["iquji-agent-wire"]["version"] != version
            or agent["package"]["name"] != CRATES[1] or wire["package"]["name"] != CRATES[0]):
        raise ValueError("两个 crate 及内部版本依赖必须使用同一稳定版本。")
    return version


def replace_once(content, pattern, replacement):
    updated, count = re.subn(pattern, replacement, content, flags=re.MULTILINE)
    if count != 1:
        raise ValueError("版本字段缺失或重复，拒绝猜测修改。")
    return updated


def rewrite_versions(root: Path, version):
    if stable(version) is None:
        raise ValueError("发布版本必须是稳定版本。")
    old = package_version(root)
    replacements = {}
    for name in VERSION_FILES[:2]:
        content = (root / name).read_text()
        replacements[name] = replace_once(
            content, rf'^(version\s*=\s*"){re.escape(old)}("\s*)$', rf'\g<1>{version}\2',
        )
    replacements["Cargo.toml"] = replace_once(
        replacements["Cargo.toml"],
        rf'^(iquji-agent-wire\s*=\s*\{{\s*version\s*=\s*"){re.escape(old)}(")',
        rf'\g<1>{version}\2',
    )
    for name in VERSION_FILES[2:]:
        content = (root / name).read_text()
        expected = CRATES if name == "Cargo.lock" else CRATES[:1]
        lock = tomllib.loads(content)
        local = [entry for entry in lock["package"] if entry["name"] in expected]
        if (len(local) != len(expected)
                or any(entry["version"] != old or "source" in entry for entry in local)):
            raise ValueError("锁文件中的本仓 crate 与源码版本不一致。")
        for crate in expected:
            content = replace_once(
                content, rf'^(name = "{crate}"\nversion = "){re.escape(old)}(")',
                rf'\g<1>{version}\2',
            )
        replacements[name] = content
    for name, content in replacements.items():
        (root / name).write_text(content)
    package_version(root)
