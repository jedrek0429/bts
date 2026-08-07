#!/usr/bin/env python3
"""Generate a BTS release manifest and checksum index."""

from __future__ import annotations

import hashlib
import json
import pathlib
import re
import subprocess
import sys

BUNDLE = re.compile(
    r"^bts-(core|display|telephony|addons|cli)-v([0-9A-Za-z.-]+)-linux-(x86_64|aarch64)\.tar\.zst$"
)


def digest(path: pathlib.Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("Usage: generate-release-manifest.py VERSION ASSET_DIRECTORY")
    version = sys.argv[1].removeprefix("v")
    root = pathlib.Path(sys.argv[2])
    compatibility = json.loads(
        (pathlib.Path(__file__).resolve().parent.parent / "compatibility.json").read_text()
    )
    validation = subprocess.run(
        [pathlib.Path(__file__).with_name("release-version.py"), "classify", version],
        stdout=subprocess.DEVNULL,
    )
    if validation.returncode != 0:
        raise SystemExit("Release version is invalid")
    installer = root / "bts-install"
    licence = root / "LICENSE"
    if not installer.is_file() or not licence.is_file():
        raise SystemExit("bts-install and LICENSE must exist before generating the manifest")

    components: dict[str, list[dict[str, object]]] = {}
    for path in sorted(root.iterdir()):
        match = BUNDLE.fullmatch(path.name)
        if not match:
            continue
        component, asset_version, architecture = match.groups()
        if asset_version != version:
            raise SystemExit(f"Asset {path.name} does not match release {version}")
        components.setdefault(component, []).append(
            {
                "platform": "linux",
                "architecture": architecture,
                "filename": path.name,
                "sha256": digest(path),
                "bundle_format_version": compatibility["component_bundle_format"],
            }
        )
    if not components:
        raise SystemExit("No portable component bundles were found")

    manifest = {
        "schema_version": compatibility["release_manifest_schema"],
        "release_version": version,
        "installer": {"filename": "bts-install", "sha256": digest(installer)},
        "components": components,
        "licence_asset": {"filename": "LICENSE", "sha256": digest(licence)},
    }
    manifest_path = root / "release-manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    (root / "bts-install.sha256").write_text(
        f"{digest(installer)}  bts-install\n", encoding="utf-8"
    )
    checksummed = [
        path
        for path in root.iterdir()
        if path.is_file() and not path.name.endswith(".sha256") and path.name != "SHA256SUMS"
    ]
    (root / "SHA256SUMS").write_text(
        "".join(f"{digest(path)}  {path.name}\n" for path in sorted(checksummed)),
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
