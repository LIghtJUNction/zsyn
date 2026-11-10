#!/usr/bin/env python3
"""Check dependency, no_std, and distributable package boundaries."""

import json
from pathlib import Path
import subprocess
import tarfile
import tomllib


def check_manifest(manifest):
    for key, value in manifest.items():
        if key in {"dependencies", "dev-dependencies", "build-dependencies"}:
            assert not value, f"dependency table must be empty: {key}"
        elif isinstance(value, dict):
            check_manifest(value)


def main():
    manifest = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))
    check_manifest(manifest)
    assert not Path("build.rs").exists(), "build.rs is forbidden"
    assert manifest["package"].get("build", False) is False, "build script is forbidden"
    assert manifest.get("features", {}).get("default", []) == [], "default core must not use alloc"
    source = Path("src/lib.rs").read_text(encoding="utf-8")
    assert "#![no_std]" in source, "the crate must always declare #![no_std]"
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--offline", "--all-features", "--format-version", "1"
    ], text=True))
    assert len(metadata["packages"]) == 1, "the crate dependency tree must contain only zsyn"
    assert not metadata["packages"][0]["dependencies"], "Cargo reports dependencies"
    assert len(metadata["workspace_members"]) == 1, "comparison must be a separate workspace"
    version = manifest["package"]["version"]
    package = Path(metadata["target_directory"]) / "package" / f"zsyn-{version}.crate"
    assert package.exists(), "run cargo package before check-package.py"
    with tarfile.open(package) as archive:
        names = {member.name.split("/", 1)[1] for member in archive.getmembers()}
        assert {"Cargo.toml", "README.md", "src/lib.rs"} <= names
        assert any(name.startswith("LICENSE") for name in names), "license file is missing"
        assert not any(name.startswith(("comparison/", ".github/", "scripts/", "tests/", "benches/"))
                       for name in names), "test or tooling files leaked into the crate"
        assert "build.rs" not in names
        with archive.extractfile(f"zsyn-{version}/Cargo.toml") as file:
            check_manifest(tomllib.loads(file.read().decode()))
    print(f"zsyn {version}: zero dependencies, no build script, isolated comparison, package verified")


if __name__ == "__main__":
    main()
