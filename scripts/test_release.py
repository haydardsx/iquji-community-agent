"""注册表确认必须能够区分待可读、内容冲突与超时。"""

import hashlib
import io
from pathlib import Path
import shutil
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import release


class RegistryConfirmationTests(unittest.TestCase):
    def test_waits_for_readable_matching_artifact(self):
        with patch.object(release, "registry_version", side_effect=[None, {"checksum": "expected"}]) as lookup:
            with patch.object(release.time, "sleep"):
                release.verify_published("iquji-agent-wire", "0.1.1", "expected")
        self.assertEqual(lookup.call_count, 2)

    def test_existing_different_artifact_fails_closed(self):
        with patch.object(release, "registry_version", return_value={"checksum": "different"}):
            with self.assertRaisesRegex(RuntimeError, "内容不同"):
                release.verify_published("iquji-agent-wire", "0.1.1", "expected")

    def test_unreadable_registry_cannot_mark_complete(self):
        with patch.object(release, "registry_version", return_value=None):
            with self.assertRaisesRegex(RuntimeError, "保留版本"):
                release.verify_published("iquji-agent-wire", "0.1.1", "expected", timeout=0)


class PartialPublicationTests(unittest.TestCase):
    def test_existing_wire_is_skipped_and_agent_is_confirmed_before_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "protocol").mkdir()
            for name in ("Cargo.toml", "protocol/Cargo.toml"):
                shutil.copyfile(release.ROOT / name, root / name)
            version = release.package_version(root)
            artifacts = root / "target/package"
            artifacts.mkdir(parents=True)
            checksums = {}
            for name in ("iquji-agent-wire", "iquji-shared-agent"):
                artifact = artifacts / f"{name}-{version}.crate"
                with tarfile.open(artifact, "w:gz") as archive:
                    info = tarfile.TarInfo(f"{name}-{version}/src/lib.rs")
                    info.size = 0
                    archive.addfile(info, io.BytesIO(b""))
                checksums[name] = hashlib.sha256(artifact.read_bytes()).hexdigest()
            published = {"iquji-agent-wire": checksums["iquji-agent-wire"]}
            commands = []

            def command(*args):
                commands.append(args)
                if args[:2] == ("cargo", "publish"):
                    self.assertEqual(args[-1], str(root / "Cargo.toml"))
                    published["iquji-shared-agent"] = checksums["iquji-shared-agent"]

            def lookup(name, _version):
                return {"checksum": published[name]} if name in published else None

            with patch.object(release, "ROOT", root), patch.object(release, "PACKAGES", [root / "protocol", root]):
                with patch.object(release, "run", side_effect=command), patch.object(release, "registry_version", side_effect=lookup):
                    with patch.object(release.sys, "argv", ["release.py"]), patch.dict(release.os.environ, {"GITHUB_REF_TYPE": "branch"}):
                        release.main()
                        self.assertEqual(len([item for item in commands if item[:2] == ("cargo", "publish")]), 1)
                        commands.clear()
                        release.main()
                        self.assertFalse(any(item[:2] == ("cargo", "publish") for item in commands))


if __name__ == "__main__":
    unittest.main()
