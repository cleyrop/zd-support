#!/usr/bin/env python3
"""Sign a CLI in a temporary keychain, notarize its ZIP, and remove credentials."""
import base64
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
import tempfile

from release import archive

REQUIRED = (
    "APPLE_DEVELOPER_ID_APPLICATION_P12", "APPLE_DEVELOPER_ID_PASSWORD",
    "APPLE_NOTARY_KEY_ID", "APPLE_NOTARY_KEY_ISSUER", "APPLE_NOTARY_KEY_P8",
)


def run(args, *, allow_failure=False):
    result = subprocess.run(args, text=True, capture_output=True)
    if result.returncode and not allow_failure:
        # Arguments may contain credential identifiers; never print the command.
        detail = ""
        if args[0] == "codesign":
            # Only report known diagnostics, never arbitrary command output.
            for diagnostic in ("errSecInternalComponent", "unable to build chain", "User interaction is not allowed", "The specified item could not be found"):
                if diagnostic in result.stderr:
                    detail += f"; {diagnostic}"
        raise RuntimeError(f"{Path(args[0]).name} failed (exit {result.returncode}){detail}")
    return result


def decode(value):
    return base64.b64decode("".join(value.split()), validate=True)


def sign(target):
    if target not in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
        raise ValueError("Expected a macOS target")
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("GITHUB_REPOSITORY") != "cleyrop/zd-support":
        raise ValueError("Signing is restricted to the zd-support release workflow")
    if not all(os.environ.get(name) for name in REQUIRED):
        raise ValueError("Missing Apple signing or notarization secrets")
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="zd-signing-", dir=os.environ["RUNNER_TEMP"]) as temp:
        root = Path(temp)
        keychain = root / "signing.keychain-db"
        p12, p8 = root / "certificate.p12", root / "notary.p8"
        original_search = shlex.split(run(["security", "list-keychains", "-d", "user"]).stdout)
        original_default = shlex.split(run(["security", "default-keychain", "-d", "user"]).stdout)
        try:
            password = secrets.token_urlsafe(32)
            run(["security", "create-keychain", "-p", password, str(keychain)])
            run(["security", "set-keychain-settings", "-lut", "3600", str(keychain)])
            run(["security", "unlock-keychain", "-p", password, str(keychain)])
            # codesign's certificate-chain lookup also uses the user search list,
            # even when its signing identity is selected with --keychain.
            run(["security", "list-keychains", "-d", "user", "-s", str(keychain), *original_search])
            run(["security", "default-keychain", "-d", "user", "-s", str(keychain)])
            p12.write_bytes(decode(os.environ[REQUIRED[0]]))
            run(["security", "import", str(p12), "-k", str(keychain), "-P", os.environ[REQUIRED[1]], "-T", "/usr/bin/codesign", "-f", "pkcs12"])
            p12.unlink()
            for certificate in sorted(Path("packaging/certs").glob("*.cer")):
                result = run(["security", "import", str(certificate), "-k", str(keychain), "-t", "cert"], allow_failure=True)
                if result.returncode and "already exists" not in result.stderr:
                    raise RuntimeError("Could not import the public Apple certificate chain")
            run(["security", "set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, str(keychain)])
            identities = run(["security", "find-identity", "-v", "-p", "codesigning", str(keychain)]).stdout
            matches = re.findall(r'\b([A-F0-9]{40})\s+"Developer ID Application:', identities)
            if len(matches) != 1:
                raise ValueError("Expected exactly one valid Developer ID Application identity")
            binary = Path("dist/payload/zd-support")
            binary.chmod(0o755)
            run(["codesign", "--force", "--options", "runtime", "--timestamp", "--identifier", "com.cleyrop.zd-support", "--keychain", str(keychain), "--sign", matches[0], str(binary)])
            run(["codesign", "--verify", "--strict", str(binary)])
            output = archive(target)
            p8.write_bytes(decode(os.environ["APPLE_NOTARY_KEY_P8"]))
            result = run(["xcrun", "notarytool", "submit", str(output), "--key", str(p8), "--key-id", os.environ["APPLE_NOTARY_KEY_ID"], "--issuer", os.environ["APPLE_NOTARY_KEY_ISSUER"], "--wait", "--timeout", "20m", "--output-format", "json"], allow_failure=True)
            response = json.loads(result.stdout) if result.stdout.strip().startswith("{") else {}
            if result.returncode or response.get("status") != "Accepted":
                raise RuntimeError("Apple notarization was not accepted; no artifact will be published")
            print(f"Signed and notarized {output.name}; submission {response['id']}.")
        finally:
            p12.unlink(missing_ok=True)
            p8.unlink(missing_ok=True)
            # Attempt every cleanup step even if restoring one setting fails.
            cleanup_errors = []
            for args in (
                ["security", "default-keychain", "-d", "user", "-s", *original_default],
                ["security", "list-keychains", "-d", "user", "-s", *original_search],
                ["security", "delete-keychain", str(keychain)],
            ):
                if args[1] == "delete-keychain" and not keychain.exists():
                    continue
                result = run(args, allow_failure=True)
                if result.returncode:
                    cleanup_errors.append(args[1])
            if cleanup_errors:
                raise RuntimeError("Keychain cleanup failed: " + ", ".join(cleanup_errors))


if __name__ == "__main__":
    try:
        sign(sys.argv[1])
    except Exception as error:
        # Do not render exception arguments: dependency errors can contain input.
        detail = str(error) if isinstance(error, RuntimeError) else type(error).__name__
        print(f"macOS signing failed: {detail}. No release artifact will be published.", file=sys.stderr)
        sys.exit(1)
