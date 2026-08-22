#!/usr/bin/env python3
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def require(text: str, needle: str, context: str) -> None:
    if needle not in text:
        raise SystemExit(f"missing {context}: {needle!r}")


def forbid(text: str, needle: str, context: str) -> None:
    if needle in text:
        raise SystemExit(f"unexpected {context}: {needle!r}")


ci = read(".github/workflows/ci.yml")
artifacts = read(".github/workflows/release-artifacts.yml")
prepare = read(".github/workflows/prepare-release-candidate.yml")
versioning = read("docs/versioning.md")

temporary_completion_tools = [
    *ROOT.glob(".github/workflows/*release-completion*"),
    *ROOT.glob("scripts/*release-completion*"),
]
if temporary_completion_tools:
    rendered = ", ".join(str(path.relative_to(ROOT)) for path in temporary_completion_tools)
    raise SystemExit(f"temporary release-completion tooling remains: {rendered}")

if (ROOT / ".github/workflows/publish-release-candidate.yml").exists():
    raise SystemExit(
        "duplicate candidate publisher remains outside the release-branch CI dependency graph"
    )

# Release-branch CI owns publication. This keeps validation and publication in
# one workflow graph instead of relying on a second workflow_run event.
require(ci, "inspect-candidate:", "candidate inspection job")
require(ci, "publish-candidate:", "candidate publication job")
require(ci, "needs: [commit-messages, rust, deployment-files, linux-aarch64]", "publication validation gate")
require(ci, "github.event_name == 'push'", "release push publication guard")
require(ci, "uses: ./.github/workflows/release-artifacts.yml", "release artifact publisher")

# AArch64 is a first-class Linux target, not a display-only special case.
require(ci, "linux-aarch64:", "ARM64 CI job")
require(ci, "cargo build --locked --release --workspace", "full ARM64 workspace build")
for component in ("core", "display", "telephony", "addons", "cli"):
    require(ci, f"bts-{component}-v*-linux-aarch64.tar.zst", f"ARM64 {component} asset validation")
require(ci, 'test -s "$assets/bts-install-linux-aarch64"', "ARM64 installer validation")

require(artifacts, "linux-x86_64:", "x86_64 release job")
require(artifacts, "linux-aarch64:", "ARM64 release job")
for architecture in ("x86_64", "aarch64"):
    require(artifacts, f'name: bts-linux-{architecture}', f"{architecture} artifact name")
    require(artifacts, f'scripts/build-release installer {architecture}', f"{architecture} installer build")
for obsolete in ("linux-aarch64-display-cli-and-installer", "bts-linux-aarch64-display-cli-and-installer"):
    forbid(artifacts, obsolete, "special-purpose ARM64 release naming")

# Preparation PRs remain unmergeable drafts until their explicitly dispatched
# canonical CI run succeeds.
require(prepare, "gh pr create \\\n            --draft", "draft release preparation PR")
require(prepare, 'gh run watch "$run_id" --exit-status', "release preparation CI wait")
require(prepare, 'gh pr ready "$pr_url"', "release preparation readiness transition")

# The operator guide must describe the same two-stage candidate contract as
# the workflow: preparation opens a draft PR, while a validated release-branch
# push is the only publication trigger.
require(versioning, "**Prepare release candidate**", "candidate preparation operation")
require(versioning, "opens a draft pull request", "candidate preparation review gate")
require(versioning, "successful push to `release/X.Y.x`", "candidate publication trigger")
forbid(versioning, "**Publish release candidate** runs manually", "obsolete manual publisher")

print("release workflow contract OK")
