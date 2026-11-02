import hashlib
import os
from pathlib import Path
import tempfile
import unittest
import zipfile
from unittest.mock import patch

import release
from sign_macos import decode


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


if __name__ == "__main__":
    unittest.main()
