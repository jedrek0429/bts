# BTS release manifest schema 1

Every v0.3 tagged release contains `bts-install`, `bts-install.sha256`, `release-manifest.json`, `SHA256SUMS`, `LICENSE` and component bundles. Checksums are generated after final asset naming. CI rebuilds representative assets and verifies that every manifest entry names an existing file with the matching SHA-256 digest.

## Manifest

```json
{
  "schema_version": 1,
  "release_version": "0.3.0",
  "installer": {
    "filename": "bts-install",
    "sha256": "<64 lowercase hexadecimal characters>"
  },
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

Component keys are `core`, `display`, `telephony` and `addons`. Platform is currently `linux`; architectures are `x86_64` and `aarch64`. Unsupported component/architecture pairs are absent. The installer must not infer filenames. Schema, release-line and bundle-format mismatches are hard errors before download activation.

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
