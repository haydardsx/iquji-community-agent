"""读取两个 crate 共同使用的发布版本。"""
import re
import tomllib


CRATES = ("iquji-community-agent", "iquji-community-agent-wire")


def package_version(root):
    agent, wire = [
        tomllib.loads((root / path).read_text())
        for path in ("Cargo.toml", "protocol/Cargo.toml")
    ]
    version = agent["package"]["version"]
    if (not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version)
            or version != wire["package"]["version"]
            or version != agent["dependencies"][CRATES[1]]["version"]
            or (agent["package"]["name"], wire["package"]["name"]) != CRATES):
        raise ValueError("两个 crate 和协议依赖必须使用同一正式版本。")
    return version
