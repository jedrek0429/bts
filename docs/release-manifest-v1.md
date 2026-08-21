# BTS release manifest schema 1

Every tagged BTS release contains the legacy x86_64 `bts-install` compatibility asset, architecture-specific installer binaries and checksums, `release-manifest.json`, `SHA256SUMS`, `LICENSE` and component bundles. Checksums are generated after final asset naming. CI rebuilds representative assets and verifies that every manifest entry names an existing file with the matching SHA-256 digest.

The current manifest and bundle compatibility numbers are defined only in [`compatibility.json`](../compatibility.json). This document describes their current layouts.

## Manifest

```json
{
  "schema_version": 1,
  "release_version": "0.3.0",
  "installer": {
    "filename": "bts-install",
    "sha256": "<64 lowercase hexadecimal characters>"
  },
  "installers": [
    {
      "platform": "linux",
      "architecture": "x86_64",
      "filename": "bts-install-linux-x86_64",
      "sha256": "<64 lowercase hexadecimal characters>"
    },
    {
      "platform": "linux",
      "architecture": "aarch64",
      "filename": "bts-install-linux-aarch64",
      "sha256": "<64 lowercase hexadecimal characters>"
    }
  ],
  "components": {
    "display": [
      {
        "platform": "linux",
        "architecture": "aarch64",
        "filename": "bts-display-v0.3.0-linux-aarch64.tar.zst",
        "sha256": "<64 lowercase hexadecimal characters>",
        "bundle_format_version": 1
      }
    ]
  },
  "licence_asset": {
    "filename": "LICENSE",
    "sha256": "<64 lowercase hexadecimal characters>"
  }
}
```

The singular `installer` entry is retained as the historical x86_64 compatibility asset so Installer v2 binaries published before architecture-specific selection can still consume newer 0.3.x releases. New installers select from `installers` using their running architecture and refuse self-update when a matching entry is absent. This prevents an ARM64 host from atomically replacing itself with an x86_64 executable.

Component keys are `core`, `display`, `telephony`, `addons` and `cli`. Platform is currently `linux`; architectures are `x86_64` and `aarch64`. Unsupported component/architecture pairs are absent. The installer must not infer filenames. Schema and bundle-format mismatches are hard errors before download activation.

`release_version` is SemVer without build metadata and must match the GitHub tag after its leading `v` is removed. Stable and prerelease versions use the same schema.

## Portable bundle format 1

Each `.tar.zst` has exactly one component root:

```text
bts-display/
├── bin/bts-display
├── systemd/bts-display.service
├── systemd/bts.target
├── systemd/bts-server.target
├── systemd/bts-display.target
├── config/display.env.example
├── install/component.conf
├── LICENSE
└── VERSION
```

`component.conf` is an environment-style metadata file identifying the component, format, abstract runtime dependencies, service and configuration filename. Distribution package names are not permitted there. `VERSION` contains the release version without a leading `v`. `LICENSE` is the complete GPL version 3 text.

Archives are deterministic: sorted members, epoch timestamps and numeric root ownership. Consumers must verify the asset checksum before extraction and reject absolute paths, `..` traversal, escaping links and special member types.

The `cli` bundle is deliberately service-less: it contains `bin/btscli`,
metadata, licence and version files but no systemd unit or component environment.
Activation publishes the executable at `/usr/bin/btscli`. It can therefore be
installed independently with `bts-install install custom --component cli`.
