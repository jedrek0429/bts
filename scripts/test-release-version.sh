#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
tool="$repository_root/scripts/release-version.py"

[[ $($tool classify 0.4.0-dev.0) == development ]]
[[ $($tool classify 0.4.0-rc.1) == candidate ]]
[[ $($tool classify 0.4.0-beta.1) == prerelease ]]
[[ $($tool classify 0.4.0) == stable ]]

$tool check-branch release/0.4.x 0.4.1-rc.2
$tool check-candidate 0.4.0-rc.1
$tool check-candidate 0.4.0-rc.2 v0.4.0-rc.1
$tool check-workspace
workspace_version=$($tool workspace-version)
$tool classify "$workspace_version" >/dev/null

if $tool check-branch release/0.4.x 0.5.0-rc.1 2>/dev/null; then
    echo "mismatched release branch was accepted" >&2
    exit 1
fi
if $tool classify 0.4 2>/dev/null; then
    echo "invalid semantic version was accepted" >&2
    exit 1
fi
if $tool check-candidate 0.4.0-rc.3 v0.4.0-rc.1 2>/dev/null; then
    echo "non-sequential release candidate was accepted" >&2
    exit 1
fi
