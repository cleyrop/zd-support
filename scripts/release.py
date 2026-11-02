#!/usr/bin/env python3
"""Build release archives from an explicit payload; never package local state."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib
import zipfile

TARGETS = (
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu",
)


def version():
    return tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]


def validate_tag(tag, expected):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", tag) or tag != f"v{expected}":
        raise ValueError("Tag must match the Cargo.toml version exactly")


def archive_name(target):
    if target not in TARGETS:
        raise ValueError("Unsupported release target")
    suffix = "zip" if "apple" in target else "tar.gz"
    return f"zd-support-v{version()}-{target}.{suffix}"


def stage(target):
    archive_name(target)  # validate before using a target in a filesystem path
    root = Path("dist/payload")
    if root.exists():
        raise ValueError("Payload already exists; use a clean build directory")
    root.mkdir(parents=True)
    shutil.copy2(Path("target") / target / "release/zd-support", root / "zd-support")
    (root / "zd-support").chmod(0o755)
    for name in ("LICENSE", "README.md"):
        shutil.copy2(name, root / name)
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
        check=True, capture_output=True, text=True,
    )
    metadata = json.loads(result.stdout)
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    inventory = []
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] not in resolved or package["source"] is None:
            continue
        name = f"{package['name']}-{package['version']}"
        base = Path(package["manifest_path"]).parent.resolve()
        license_file = package.get("license_file")
        supplied = (base / license_file).resolve() if license_file else None
        inventory.append({k: package.get(k) for k in ("name", "version", "license", "repository", "source")})
        for file in base.rglob("*"):
            if file.is_symlink() or not file.is_file() or not file.resolve().is_relative_to(base):
                continue
            relative = file.relative_to(base)
            is_notice = file.name.lower().startswith(("license", "licence", "copying", "notice", "copyright")) or file.resolve() == supplied
            if is_notice:
                destination = root / "third-party-licenses" / name / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(file, destination)
            # Ship the corresponding source for MPL dependencies as well as notices.
            if "MPL-2.0" in (package.get("license") or ""):
                destination = root / "third-party-sources" / name / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(file, destination)
    (root / "THIRD_PARTY_NOTICES.json").write_text(json.dumps(inventory, indent=2) + "\n")


def archive(target):
    filename = archive_name(target)
    prefix = filename.removesuffix(".zip").removesuffix(".tar.gz")
    root = Path("dist/payload")
    if not (root / "zd-support").is_file():
        raise ValueError("Missing release binary")
    output = Path("dist") / filename
    if output.exists():
        raise ValueError("Refusing to replace an existing release archive")
    if filename.endswith(".zip"):
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as target_zip:
            for file in sorted(root.rglob("*")):
                if file.is_file():
                    target_zip.write(file, f"{prefix}/{file.relative_to(root)}")
    else:
        with tarfile.open(output, "w:gz") as target_tar:
            target_tar.add(root, arcname=prefix)
    return output


def checksums(directory):
    root = Path(directory)
    expected = {archive_name(target) for target in TARGETS}
    actual = {p.name for p in root.iterdir() if p.is_file()}
    if actual != expected:
        raise ValueError("Release must contain exactly the four expected archives")
    lines = [f"{hashlib.sha256((root / name).read_bytes()).hexdigest()}  {name}" for name in sorted(expected)]
    (root / "SHA256SUMS").write_text("\n".join(lines) + "\n")


def main():
    command, *args = sys.argv[1:]
    if command == "validate-tag" and len(args) == 1:
        validate_tag(args[0], version())
    elif command == "stage" and len(args) == 1:
        stage(args[0])
    elif command == "archive" and len(args) == 1:
        print(archive(args[0]))
    elif command == "checksums" and len(args) == 1:
        checksums(args[0])
    else:
        raise ValueError("Invalid release command")


if __name__ == "__main__":
    main()
