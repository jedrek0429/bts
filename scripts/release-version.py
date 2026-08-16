#!/usr/bin/env python3
"""Validate and mutate BTS product versions and release branches."""

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


def set_workspace_version(root: pathlib.Path, value: str) -> None:
    value = value.removeprefix("v")
    parse_version(value)
    path = root / "Cargo.toml"
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    in_workspace_package = False
    changed = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_workspace_package = stripped == "[workspace.package]"
            continue
        if in_workspace_package and re.fullmatch(r'version\s*=\s*"[^"]+"', stripped):
            suffix = "\n" if line.endswith("\n") else ""
            lines[index] = f'version = "{value}"{suffix}'
            changed = True
            break
    if not changed:
        raise SystemExit("Cargo.toml has no [workspace.package] version")
    path.write_text("".join(lines), encoding="utf-8")


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


def release_branch(value: str) -> str:
    match = parse_version(value)
    return f"release/{match.group(1)}.{match.group(2)}.x"


def candidate_numbers(value: str, tags: list[str]) -> list[int]:
    match = parse_version(value)
    prefix = f"v{match.group(1)}.{match.group(2)}.{match.group(3)}-rc."
    return sorted(
        {
            int(tag.removeprefix(prefix))
            for tag in tags
            if tag.startswith(prefix) and tag.removeprefix(prefix).isdigit()
        }
    )


def check_candidate(value: str, tags: list[str]) -> None:
    match = parse_version(value)
    prerelease = match.group(4)
    candidate = re.fullmatch(r"rc\.([1-9]\d*)", prerelease or "")
    if not candidate:
        raise SystemExit(f"Version {value} is not a release candidate")
    numbers = candidate_numbers(value, tags)
    number = int(candidate.group(1))
    if number in numbers:
        return
    expected = max(numbers, default=0) + 1
    if number != expected:
        prefix = f"v{match.group(1)}.{match.group(2)}.{match.group(3)}-rc."
        raise SystemExit(f"Expected {prefix}{expected}, got v{value}")


def next_candidate(value: str, tags: list[str]) -> str:
    match = parse_version(value)
    kind = classify(value)
    if kind not in {"development", "candidate"}:
        raise SystemExit(f"Cannot derive a release candidate from {value}")
    numbers = candidate_numbers(value, tags)
    expected = max(numbers, default=0) + 1
    if kind == "candidate":
        current = int((match.group(4) or "").removeprefix("rc."))
        if current not in numbers:
            if current != expected:
                raise SystemExit(
                    f"Unpublished candidate {value} is out of sequence; expected rc.{expected}"
                )
            return value.removeprefix("v")
    return f"{match.group(1)}.{match.group(2)}.{match.group(3)}-rc.{expected}"


def stable_version(value: str) -> str:
    match = parse_version(value)
    if classify(value) != "candidate":
        raise SystemExit(f"Stable promotion requires a release candidate, got {value}")
    return f"{match.group(1)}.{match.group(2)}.{match.group(3)}"


def next_patch_development(value: str) -> str:
    match = parse_version(value)
    if classify(value) != "stable":
        raise SystemExit(f"Patch development requires a stable version, got {value}")
    return f"{match.group(1)}.{match.group(2)}.{int(match.group(3)) + 1}-dev.0"


def next_minor_development(value: str) -> str:
    match = parse_version(value)
    if classify(value) != "stable":
        raise SystemExit(f"Minor development requires a stable version, got {value}")
    return f"{match.group(1)}.{int(match.group(2)) + 1}.0-dev.0"


def main() -> int:
    if len(sys.argv) < 2:
        raise SystemExit(
            "Usage: release-version.py "
            "workspace-version|set-version|check-workspace|classify|check-branch|"
            "release-branch|check-candidate|next-candidate|stable-version|"
            "next-patch-development|next-minor-development [VALUE] [TAGS...]"
        )
    root = pathlib.Path(__file__).resolve().parent.parent
    command = sys.argv[1]
    if command == "workspace-version" and len(sys.argv) == 2:
        print(workspace_version(root))
    elif command == "set-version" and len(sys.argv) == 3:
        set_workspace_version(root, sys.argv[2])
    elif command == "check-workspace" and len(sys.argv) == 2:
        check_workspace(root)
    elif command == "classify" and len(sys.argv) == 3:
        print(classify(sys.argv[2]))
    elif command == "check-branch" and len(sys.argv) == 4:
        check_branch(sys.argv[2], sys.argv[3])
    elif command == "release-branch" and len(sys.argv) == 3:
        print(release_branch(sys.argv[2]))
    elif command == "check-candidate" and len(sys.argv) >= 3:
        check_candidate(sys.argv[2], sys.argv[3:])
    elif command == "next-candidate" and len(sys.argv) >= 3:
        print(next_candidate(sys.argv[2], sys.argv[3:]))
    elif command == "stable-version" and len(sys.argv) == 3:
        print(stable_version(sys.argv[2]))
    elif command == "next-patch-development" and len(sys.argv) == 3:
        print(next_patch_development(sys.argv[2]))
    elif command == "next-minor-development" and len(sys.argv) == 3:
        print(next_minor_development(sys.argv[2]))
    else:
        raise SystemExit(f"Invalid arguments for {command}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
