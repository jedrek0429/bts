#!/usr/bin/env python3
"""Validate BTS product versions and release branches."""

from __future__ import annotations

import pathlib
import re
import sys
import tomllib

SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?$"
)
RELEASE_BRANCH = re.compile(r"^(?:refs/heads/)?release/(0|[1-9]\d*)\.(0|[1-9]\d*)\.x$")


def parse_version(value: str) -> re.Match[str]:
    match = SEMVER.fullmatch(value.removeprefix("v"))
    if not match:
        raise SystemExit(f"Invalid BTS version: {value}")
    return match


def workspace_version(root: pathlib.Path) -> str:
    with (root / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)["workspace"]["package"]["version"]


def check_workspace(root: pathlib.Path) -> None:
    with (root / "Cargo.toml").open("rb") as handle:
        workspace = tomllib.load(handle)["workspace"]
    parse_version(workspace["package"]["version"])
    for member in workspace["members"]:
        with (root / member / "Cargo.toml").open("rb") as handle:
            package = tomllib.load(handle)["package"]
        if package.get("version") != {"workspace": True}:
            raise SystemExit(f"{member} must inherit the workspace version")


def classify(value: str) -> str:
    prerelease = parse_version(value).group(4)
    if prerelease is None:
        return "stable"
    if re.fullmatch(r"rc\.[1-9]\d*", prerelease):
        return "candidate"
    if re.fullmatch(r"dev\.(0|[1-9]\d*)", prerelease):
        return "development"
    return "prerelease"


def check_branch(branch: str, value: str) -> None:
    branch_match = RELEASE_BRANCH.fullmatch(branch)
    if not branch_match:
        raise SystemExit(f"Invalid release branch: {branch}")
    version_match = parse_version(value)
    if branch_match.groups() != version_match.groups()[:2]:
        raise SystemExit(f"Version {value} does not belong to {branch}")


def check_candidate(value: str, tags: list[str]) -> None:
    match = parse_version(value)
    prerelease = match.group(4)
    candidate = re.fullmatch(r"rc\.([1-9]\d*)", prerelease or "")
    if not candidate:
        raise SystemExit(f"Version {value} is not a release candidate")
    prefix = f"v{match.group(1)}.{match.group(2)}.{match.group(3)}-rc."
    numbers = [
        int(tag.removeprefix(prefix))
        for tag in tags
        if tag.startswith(prefix) and tag.removeprefix(prefix).isdigit()
    ]
    number = int(candidate.group(1))
    if number in numbers:
        return
    expected = max(numbers, default=0) + 1
    if number != expected:
        raise SystemExit(f"Expected {prefix}{expected}, got v{value}")


def main() -> int:
    if len(sys.argv) < 2:
        raise SystemExit(
            "Usage: release-version.py "
            "workspace-version|check-workspace|classify|check-branch|check-candidate [VALUE]"
        )
    root = pathlib.Path(__file__).resolve().parent.parent
    command = sys.argv[1]
    if command == "workspace-version" and len(sys.argv) == 2:
        print(workspace_version(root))
    elif command == "check-workspace" and len(sys.argv) == 2:
        check_workspace(root)
    elif command == "classify" and len(sys.argv) == 3:
        print(classify(sys.argv[2]))
    elif command == "check-branch" and len(sys.argv) == 4:
        check_branch(sys.argv[2], sys.argv[3])
    elif command == "check-candidate" and len(sys.argv) >= 3:
        check_candidate(sys.argv[2], sys.argv[3:])
    else:
        raise SystemExit(f"Invalid arguments for {command}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
