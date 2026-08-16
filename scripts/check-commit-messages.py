#!/usr/bin/env python3
"""Validate Conventional Commit subjects in a Git revision range."""

from __future__ import annotations

import re
import subprocess
import sys

SUBJECT = re.compile(
    r"^(build|chore|ci|docs|feat|fix|perf|refactor|revert|style|test)"
    r"(?:\([a-z0-9][a-z0-9._/-]*\))?!?: .+"
)
ZERO_SHA = re.compile(r"^0+$")
SCRIPT_PATH = "scripts/check-commit-messages.py"


def is_ancestor(ancestor: str, descendant: str) -> bool:
    return (
        subprocess.run(
            ["git", "merge-base", "--is-ancestor", ancestor, descendant],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode
        == 0
    )


def enforcement_base(base: str, head: str) -> str:
    """Keep commits predating this validator outside the enforcement range."""
    introductions = subprocess.check_output(
        ["git", "log", "--diff-filter=A", "--format=%H", "--reverse", head, "--", SCRIPT_PATH],
        text=True,
    ).splitlines()
    if not introductions:
        return base

    introduction = introductions[0]
    if is_ancestor(introduction, base):
        return base

    return f"{introduction}^"


def commits(base: str, head: str) -> list[tuple[str, str]]:
    if ZERO_SHA.fullmatch(base):
        base = f"{head}^"
    base = enforcement_base(base, head)
    output = subprocess.check_output(
        ["git", "log", "--no-merges", "--format=%H%x00%s", f"{base}..{head}"],
        text=True,
    )
    result: list[tuple[str, str]] = []
    for line in output.splitlines():
        if not line:
            continue
        sha, subject = line.split("\0", 1)
        result.append((sha, subject))
    return result


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("Usage: check-commit-messages.py BASE HEAD")

    invalid = [
        (sha, subject)
        for sha, subject in commits(sys.argv[1], sys.argv[2])
        if SUBJECT.fullmatch(subject) is None
    ]
    if not invalid:
        return 0

    print("Commit subjects must use Conventional Commits:", file=sys.stderr)
    print("  <type>[optional scope][!]: <description>", file=sys.stderr)
    print(
        "  allowed types: build, chore, ci, docs, feat, fix, perf, refactor, revert, style, test",
        file=sys.stderr,
    )
    for sha, subject in invalid:
        print(f"  {sha[:12]}  {subject}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
