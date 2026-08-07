#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
assets="$test_root/assets"
install -d "$assets" "$test_root/bin"
version=$("$repository_root/scripts/release-version.py" workspace-version)
for component in core display telephony addons cli; do
    binary="bts-$component"
    [[ "$component" == cli ]] && binary=btscli
    install -m755 /usr/bin/true "$test_root/bin/$binary"
    "$repository_root/scripts/build-release" component "$component" x86_64 "$test_root/bin/$binary" "$assets" >/dev/null
done
"$repository_root/scripts/build-release" component display aarch64 "$test_root/bin/bts-display" "$assets" >/dev/null
"$repository_root/scripts/build-release" installer /usr/bin/true "$assets"
"$repository_root/scripts/build-release" assemble "$assets" >/dev/null

python3 - "$assets" "$version" "$repository_root/compatibility.json" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
compatibility = json.loads(pathlib.Path(sys.argv[3]).read_text())
manifest = json.loads((root / "release-manifest.json").read_text())
assert manifest["schema_version"] == compatibility["release_manifest_schema"]
assert manifest["release_version"] == sys.argv[2]
assert {"core", "display", "telephony", "addons", "cli"} == set(manifest["components"])
assert {item["architecture"] for item in manifest["components"]["display"]} == {"x86_64", "aarch64"}
for item in [manifest["installer"], manifest["licence_asset"], *[asset for assets in manifest["components"].values() for asset in assets]]:
    path = root / item["filename"]
    assert path.is_file(), item["filename"]
    assert hashlib.sha256(path.read_bytes()).hexdigest() == item["sha256"]
for assets in manifest["components"].values():
    assert all(item["bundle_format_version"] == compatibility["component_bundle_format"] for item in assets)
checksums = dict(line.split(maxsplit=1)[::-1] for line in (root / "SHA256SUMS").read_text().splitlines())
for filename, checksum in checksums.items():
    assert hashlib.sha256((root / filename).read_bytes()).hexdigest() == checksum
PY
