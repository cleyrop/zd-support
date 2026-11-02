import hashlib
import os
from pathlib import Path
import tempfile
import subprocess
import unittest
import zipfile
from unittest.mock import patch

import release
from sign_macos import decode
import sign_macos


class ReleaseTests(unittest.TestCase):
    def test_release_tag_must_match_version_without_shell_payload(self):
        release.validate_tag("v0.3.5", "0.3.5")
        for bad in ("main", "v0.3.6", "v0.3.5;touch /tmp/x", "v0.3.5/../other"):
            with self.assertRaises(ValueError):
                release.validate_tag(bad, "0.3.5")

    def test_release_requires_exactly_four_platforms(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(release, "version", return_value="0.3.5"):
            root = Path(tmp)
            with self.assertRaises(ValueError):
                release.checksums(root)
            for target in release.TARGETS:
                (root / release.archive_name(target)).write_bytes(target.encode())
            release.checksums(root)
            for line in (root / "SHA256SUMS").read_text().splitlines():
                digest, name = line.split("  ")
                self.assertEqual(digest, hashlib.sha256((root / name).read_bytes()).hexdigest())
            (root / "SHA256SUMS").unlink()
            (root / "unexpected.txt").write_text("must not publish")
            with self.assertRaises(ValueError):
                release.checksums(root)

    def test_archive_only_contains_staged_payload_and_preserves_execute_permission(self):
        old = Path.cwd()
        with tempfile.TemporaryDirectory() as tmp, patch.object(release, "version", return_value="0.3.5"):
            try:
                os.chdir(tmp)
                payload = Path("dist/payload")
                payload.mkdir(parents=True)
                binary = payload / "zd-support"
                binary.write_bytes(b"fake executable")
                binary.chmod(0o755)
                Path("session.json").write_text("must never enter archive")
                output = release.archive("aarch64-apple-darwin")
                with zipfile.ZipFile(output) as archive:
                    self.assertEqual(len(archive.namelist()), 1)
                    item = archive.infolist()[0]
                    self.assertTrue(item.filename.endswith("/zd-support"))
                    self.assertEqual((item.external_attr >> 16) & 0o777, 0o755)
                with self.assertRaises(ValueError):
                    release.archive("aarch64-apple-darwin")
            finally:
                os.chdir(old)

    def test_base64_credentials_accept_wrapped_exports_without_altering_bytes(self):
        self.assertEqual(decode(" ZmFr\nZSBvbmx5\n"), b"fake only")
        with self.assertRaises(ValueError):
            decode("invalid!!")

    def test_signing_failure_restores_keychains_and_removes_credentials(self):
        for restoration_fails in (False, True):
            with self.subTest(restoration_fails=restoration_fails), tempfile.TemporaryDirectory() as tmp:
                calls = []
                def command(args, **kwargs):
                    calls.append(args)
                    stdout, stderr, code = "", "", 0
                    if args[:2] == ["security", "list-keychains"] and "-s" not in args:
                        stdout = '"/original/login.keychain-db" " /original/with space.keychain-db"'
                    elif args[:2] == ["security", "default-keychain"] and "-s" not in args:
                        stdout = '"/original/login.keychain-db"'
                    elif args[:2] == ["security", "create-keychain"]:
                        Path(args[-1]).touch()
                    elif args[:2] == ["security", "find-identity"]:
                        stdout = '1) ' + 'A' * 40 + ' "Developer ID Application: Test"'
                    elif args[0] == "codesign":
                        code, stderr = 1, "errSecInternalComponent"
                    elif restoration_fails and args[:2] == ["security", "default-keychain"] and args[-1] == "/original/login.keychain-db":
                        code = 1
                    return subprocess.CompletedProcess(args, code, stdout, stderr)

                env = {key: "ZmFrZQ==" for key in sign_macos.REQUIRED}
                env.update(GITHUB_ACTIONS="true", GITHUB_REPOSITORY="cleyrop/zd-support", RUNNER_TEMP=tmp)
                with patch.dict(os.environ, env), patch.object(sign_macos.subprocess, "run", side_effect=command), patch.object(Path, "chmod"):
                    with self.assertRaises(RuntimeError):
                        sign_macos.sign("aarch64-apple-darwin")
                self.assertIn(["security", "default-keychain", "-d", "user", "-s", "/original/login.keychain-db"], calls)
                self.assertIn(["security", "list-keychains", "-d", "user", "-s", "/original/login.keychain-db", " /original/with space.keychain-db"], calls)
                self.assertEqual(calls[-1][1], "delete-keychain")
                self.assertEqual(list(Path(tmp).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
