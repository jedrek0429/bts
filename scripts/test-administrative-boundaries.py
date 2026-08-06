#!/usr/bin/env python3
"""Enforce the administrative crate dependency direction from issue #36."""

import json
import pathlib
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parent.parent


def fail(message: str) -> None:
    print(f"administrative boundary violation: {message}", file=sys.stderr)
    raise SystemExit(1)


metadata = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--format-version=1", "--no-deps", "--locked"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
)
packages = {package["name"]: package for package in metadata["packages"]}


def dependencies(package: str) -> set[str]:
    return {dependency["name"] for dependency in packages[package]["dependencies"]}


def forbid(package: str, forbidden: set[str]) -> None:
    invalid = dependencies(package) & forbidden
    if invalid:
        fail(f"{package} must not depend on {', '.join(sorted(invalid))}")


if "bts-protocol" not in packages:
    fail("workspace has no bts-protocol crate")

forbid(
    "bts-protocol",
    {
        "clap",
        "reqwest",
        "bts-core",
        "bts-sdk",
        "bts-cli",
        "bts-terminal",
        "bts-display",
        "bts-telephony",
        "bts-addons",
    },
)

if "bts-sdk" in packages:
    if "bts-protocol" not in dependencies("bts-sdk"):
        fail("bts-sdk must depend on bts-protocol")
    forbid(
        "bts-sdk",
        {
            "clap",
            "bts-core",
            "bts-cli",
            "bts-terminal",
            "bts-display",
            "bts-telephony",
            "bts-addons",
        },
    )

if "bts-cli" in packages:
    if "bts-sdk" not in dependencies("bts-cli"):
        fail("bts-cli must depend on bts-sdk")
    forbid(
        "bts-cli",
        {
            "bts-protocol",
            "bts-core",
            "bts-terminal",
            "bts-display",
            "bts-telephony",
            "bts-addons",
        },
    )
