#!/usr/bin/env python3
"""Validate a release candidate, publish its immutable tag, and verify the upload."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time
import tomllib
import urllib.error
import urllib.request


REPOSITORY = "LIghtJUNction/zsyn"
METADATA = ("Cargo.toml", "Cargo.lock", "comparison/Cargo.lock", "README.md")
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def version_parts(value):
    match = VERSION.fullmatch(value)
    require(match is not None, "version must be X.Y.Z without a prefix or leading zeroes")
    parts = tuple(int(part) for part in match.groups())
    require(all(part < 2**64 for part in parts), "version component exceeds u64")
    return parts


def next_version(current, bump):
    parts = list(version_parts(current))
    require(bump in {"none", "patch", "minor", "major"}, "invalid bump")
    if bump != "none":
        index = {"major": 0, "minor": 1, "patch": 2}[bump]
        parts[index] += 1
        parts[index + 1:] = [0] * (2 - index)
    candidate = ".".join(map(str, parts))
    version_parts(candidate)
    return candidate


def run(*args, capture=True, check=True):
    result = subprocess.run(args, text=True, encoding="utf-8", check=False,
                            stdout=subprocess.PIPE if capture else None)
    if check:
        require(result.returncode == 0, f"command failed: {' '.join(args)}")
    return result.stdout.strip() if check and capture else result


def git(*args):
    return run("git", *args)


def package_version():
    return tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]


def set_version(version):
    version_parts(version)
    old = package_version()
    path = Path("Cargo.toml")
    text = path.read_text(encoding="utf-8")
    section = re.search(r"(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)", text)
    require(section is not None, "missing [package] section")
    body, count = re.subn(r'(?m)^version\s*=\s*"[^"]+"', f'version = "{version}"', section[1])
    require(count == 1, "package.version must be one literal string")
    path.write_text(text[:section.start(1)] + body + text[section.end(1):], encoding="utf-8")
    for filename in ("Cargo.lock", "comparison/Cargo.lock"):
        path = Path(filename)
        if not path.exists():
            continue
        blocks = path.read_text(encoding="utf-8").split("[[package]]")
        found = 0
        for index in range(1, len(blocks)):
            if re.search(r'(?m)^name = "zsyn"$', blocks[index]):
                require(not re.search(r'(?m)^source\s*=', blocks[index]),
                        f"{filename} contains a registry zsyn, expected the path dependency")
                blocks[index], count = re.subn(r'(?m)^version = "[^"]+"$',
                                               f'version = "{version}"', blocks[index], count=1)
                found += count
        require(found == 1, f"{filename} must contain exactly one zsyn package")
        path.write_text("[[package]]".join(blocks), encoding="utf-8")
    path = Path("README.md")
    if path.exists():
        text = path.read_text(encoding="utf-8")
        text = re.sub(r'(?m)(^zsyn\s*=\s*(?:\{\s*version\s*=\s*)?")' +
                      re.escape(old) + r'(")', lambda m: m[1] + version + m[2], text)
        path.write_text(text, encoding="utf-8")


def registry_version(version):
    version_parts(version)
    request = urllib.request.Request(f"https://crates.io/api/v1/crates/zsyn/{version}",
                                     headers={"User-Agent": f"zsyn-release (github.com/{REPOSITORY})"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)["version"]
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise RuntimeError(f"registry check failed: HTTP {error.code}") from error


def trusted_branch():
    require(os.environ.get("GITHUB_EVENT_NAME") in {"push", "workflow_dispatch"},
            "releases require a default-branch manifest push or workflow_dispatch")
    require(os.environ.get("GITHUB_REPOSITORY") == REPOSITORY,
            "releases are restricted to the upstream repository")
    branch = run("gh", "api", f"repos/{REPOSITORY}", "--jq", ".default_branch")
    require(os.environ.get("GITHUB_REF") == f"refs/heads/{branch}",
            "releases are restricted to the default branch")
    git("fetch", "origin", branch, "--tags")
    return branch


def tag_exists(tag):
    return run("git", "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}", check=False).returncode == 0


def tagged_by_this_run(tag, source):
    if git("cat-file", "-t", f"refs/tags/{tag}") != "tag":
        return False
    message = git("for-each-ref", "--format=%(contents)", f"refs/tags/{tag}").splitlines()
    return (f"Source-Commit: {source}" in message and
            f"Release-Run: {os.environ.get('GITHUB_RUN_ID')}" in message)


def same_release(tag, source):
    require(tagged_by_this_run(tag, source),
            "tag already belongs to another release; never overwrite it")


def plan(bump):
    branch = trusted_branch()
    if os.environ.get("GITHUB_EVENT_NAME") == "push":
        require(bump == "none", "manifest pushes publish the committed version without a bump")
    source = git("rev-parse", "HEAD")
    version = next_version(package_version(), bump)
    tag = f"v{version}"
    tagged = tag_exists(tag)
    if not tagged:
        require(git("rev-parse", f"origin/{branch}") == source,
                "default branch advanced; dispatch a new run")
    existing = registry_version(version)
    retry = tagged and tagged_by_this_run(tag, source)
    publish = not (os.environ.get("GITHUB_EVENT_NAME") == "push" and existing is not None and not retry)
    if publish:
        if tagged:
            require(retry, "tag already belongs to another release; never overwrite it")
        else:
            require(existing is None, "version is already published; choose a bump")
    data = {"source": source, "version": version, "tag": tag,
            "publish": "true" if publish else "false"}
    print(json.dumps(data, indent=2))
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as file:
            for key, value in data.items():
                file.write(f"{key}={value}\n")


def state(**values):
    path = Path("target/release-state.json")
    path.parent.mkdir(exist_ok=True)
    data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    data.update(values)
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def verify_registry(version, checksum, attempts=1):
    for attempt in range(attempts):
        data = registry_version(version)
        if data is not None:
            require(not data.get("yanked"), "published version is yanked")
            require(data.get("checksum") == checksum, "published package differs from the release tag")
            return True
        if attempt + 1 < attempts:
            time.sleep(5)
    return False


def publish(source, version):
    require(bool(os.environ.get("CARGO_REGISTRY_TOKEN", "").strip()),
            "configure Actions secret CARGO_REGISTRY_TOKEN before publishing")
    require(re.fullmatch(r"[0-9a-f]{40}", source) is not None, "invalid source commit")
    version_parts(version)
    require(not git("status", "--porcelain"), "release checkout must be clean")
    require(git("rev-parse", "HEAD") == source, "checkout differs from the tested source")
    branch = trusted_branch()
    tag = f"v{version}"
    state(version=version, source=source, stage="preflight")
    set_version(version)
    for name in METADATA:
        if Path(name).exists():
            git("add", "--", name)
    expected_tree = git("write-tree")
    if tag_exists(tag):
        same_release(tag, source)
        require(git("rev-parse", f"{tag}^{{tree}}") == expected_tree,
                "existing tag differs from the tested candidate")
        git("checkout", "--detach", tag)
    else:
        require(git("rev-parse", f"origin/{branch}") == source,
                "default branch advanced during tests; dispatch a new run")
        require(registry_version(version) is None, "version was published during tests")
        run("cargo", "publish", "--locked", "--dry-run", "--allow-dirty", capture=False)
        git("config", "user.name", "github-actions[bot]")
        git("config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com")
        if git("diff", "--cached", "--name-only"):
            git("commit", "-m", f"chore: release {tag}")
        git("tag", "-a", tag, "-m", f"zsyn {version}\n\nSource-Commit: {source}\n"
            f"Release-Run: {os.environ['GITHUB_RUN_ID']}")
        run("git", "push", "--atomic", "origin", f"HEAD:refs/heads/{branch}",
            f"refs/tags/{tag}", capture=False)
    commit = git("rev-parse", "HEAD")
    state(stage="tagged", commit=commit, tag=tag)
    run("cargo", "package", "--locked", capture=False)
    run("python", "scripts/check-package.py", capture=False)
    metadata = json.loads(run("cargo", "metadata", "--locked", "--offline", "--no-deps",
                              "--format-version", "1"))
    package = Path(metadata["target_directory"]) / "package" / f"zsyn-{version}.crate"
    checksum = hashlib.sha256(package.read_bytes()).hexdigest()
    state(checksum=checksum)
    if not verify_registry(version, checksum):
        result = run("cargo", "publish", "--locked", capture=False, check=False)
        require(verify_registry(version, checksum, attempts=12),
                f"cargo publish exited {result.returncode}; registry upload not verified. Rerun this job.")
    state(stage="published")
    existing = run("gh", "release", "view", tag, "--repo", REPOSITORY,
                   "--json", "tagName,isDraft,url", check=False)
    if existing.returncode == 0:
        data = json.loads(existing.stdout)
        require(data["tagName"] == tag, "GitHub release tag mismatch")
        if data["isDraft"]:
            run("gh", "release", "edit", tag, "--repo", REPOSITORY, "--draft=false", capture=False)
    else:
        notes = Path("target/release-notes.md")
        notes.write_text(f"zsyn {version}\n\n"
                         f"- Crate: https://crates.io/crates/zsyn/{version}\n"
                         f"- Validated source: {commit}\n"
                         f"- Package SHA-256: `{checksum}`\n"
                         "- OS, MSRV, target, package, differential, and Miri gates passed.\n",
                         encoding="utf-8")
        run("gh", "release", "create", tag, "--repo", REPOSITORY, "--verify-tag",
            "--title", f"zsyn {version}", "--notes-file", str(notes), capture=False)
    run("gh", "release", "upload", tag, str(package), "--repo", REPOSITORY,
        "--clobber", capture=False)
    url = run("gh", "release", "view", tag, "--repo", REPOSITORY, "--json", "url", "--jq", ".url")
    state(stage="complete", release_url=url)
    print(f"Verified zsyn {version}: {url}")
    if output := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(output, "a", encoding="utf-8") as file:
            file.write(f"## zsyn {version}\n\n[GitHub Release]({url}) · "
                       f"[crates.io](https://crates.io/crates/zsyn/{version})\n\n"
                       f"Commit: `{commit}`\n\nSHA-256: `{checksum}`\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    edit = commands.add_parser("set-version")
    edit.add_argument("version")
    planning = commands.add_parser("plan")
    planning.add_argument("--bump", choices=["none", "patch", "minor", "major"], required=True)
    release = commands.add_parser("publish")
    release.add_argument("--source", required=True)
    release.add_argument("--version", required=True)
    args = parser.parse_args()
    try:
        if args.command == "set-version":
            set_version(args.version)
        elif args.command == "plan":
            plan(args.bump)
        else:
            publish(args.source, args.version)
    except (RuntimeError, OSError, ValueError) as error:
        print(f"::error::{error}")
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
