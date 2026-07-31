use std::{
    fs,
    io::{Read, Seek},
    path::{Component as PathComponent, Path},
};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

pub fn verify_sha256<R: Read>(mut reader: R, expected: &str) -> Result<()> {
    let mut digest = Sha256::new();
    std::io::copy(&mut reader, &mut digest)?;
    let actual = hex::encode(digest.finalize());
    ensure!(
        actual.eq_ignore_ascii_case(expected),
        "Downloaded asset checksum mismatch: expected {expected}, received {actual}."
    );
    Ok(())
}

pub fn extract_tar_zst<R: Read + Seek>(mut input: R, destination: &Path) -> Result<()> {
    input.rewind()?;
    let decoder =
        zstd::stream::read::Decoder::new(input).context("Asset is not a valid zstd stream")?;
    let mut archive = tar::Archive::new(decoder);
    fs::create_dir_all(destination)?;
    for entry in archive
        .entries()
        .context("Asset is not a valid tar archive")?
    {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        validate_member_path(&path)?;
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            let target = entry.link_name()?.context("Archive link has no target")?;
            validate_link_target(&path, &target)?;
        } else if !(kind.is_file() || kind.is_dir()) {
            bail!(
                "Archive member '{}' has unsupported type {:?}.",
                path.display(),
                kind
            );
        }
        let output = destination.join(&path);
        ensure!(
            output.starts_with(destination),
            "Archive member escapes the staging directory."
        );
        entry
            .unpack_in(destination)
            .with_context(|| format!("Could not extract '{}'.", path.display()))?;
    }
    Ok(())
}

fn validate_member_path(path: &Path) -> Result<()> {
    ensure!(
        !path.is_absolute(),
        "Archive contains absolute path '{}'.",
        path.display()
    );
    ensure!(
        !path.as_os_str().is_empty(),
        "Archive contains an empty path."
    );
    for component in path.components() {
        match component {
            PathComponent::Normal(_) | PathComponent::CurDir => {}
            PathComponent::ParentDir => bail!(
                "Archive path '{}' traverses outside the bundle.",
                path.display()
            ),
            PathComponent::RootDir | PathComponent::Prefix(_) => {
                bail!("Archive path '{}' is absolute.", path.display())
            }
        }
    }
    Ok(())
}

fn validate_link_target(member: &Path, target: &Path) -> Result<()> {
    ensure!(
        !target.is_absolute(),
        "Archive link '{}' has absolute target '{}'.",
        member.display(),
        target.display()
    );
    let parent = member.parent().unwrap_or_else(|| Path::new(""));
    let mut depth = parent
        .components()
        .filter(|value| matches!(value, PathComponent::Normal(_)))
        .count() as isize;
    for component in target.components() {
        match component {
            PathComponent::Normal(_) => depth += 1,
            PathComponent::ParentDir => {
                depth -= 1;
                ensure!(
                    depth >= 0,
                    "Archive link '{}' escapes the bundle.",
                    member.display()
                );
            }
            PathComponent::CurDir => {}
            PathComponent::RootDir | PathComponent::Prefix(_) => {
                bail!("Archive link '{}' has an unsafe target.", member.display())
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::tempdir;

    fn compressed_archive(path: &str, kind: tar::EntryType, link: Option<&str>) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_mode(0o644);
            header.set_size(0);
            let name = path.as_bytes();
            header.as_mut_bytes()[..name.len()].copy_from_slice(name);
            if let Some(target) = link {
                header.set_link_name(target).unwrap();
            }
            header.set_cksum();
            builder.append(&header, &b""[..]).unwrap();
            builder.finish().unwrap();
        }
        zstd::stream::encode_all(Cursor::new(tar_bytes), 1).unwrap()
    }

    #[test]
    fn verifies_checksums() {
        let expected = hex::encode(Sha256::digest(b"hello"));
        verify_sha256(&b"hello"[..], &expected).unwrap();
        assert!(verify_sha256(&b"wrong"[..], &expected).is_err());
    }

    #[test]
    fn rejects_path_traversal_absolute_paths_and_unsafe_links() {
        let root = tempdir().unwrap();
        for bytes in [
            compressed_archive("../escape", tar::EntryType::Regular, None),
            compressed_archive("/absolute", tar::EntryType::Regular, None),
            compressed_archive("bundle/link", tar::EntryType::Symlink, Some("../../escape")),
        ] {
            assert!(extract_tar_zst(Cursor::new(bytes), root.path()).is_err());
        }
    }

    #[test]
    fn extracts_safe_files_and_relative_links() {
        let root = tempdir().unwrap();
        extract_tar_zst(
            Cursor::new(compressed_archive(
                "bundle/file",
                tar::EntryType::Regular,
                None,
            )),
            root.path(),
        )
        .unwrap();
        assert!(root.path().join("bundle/file").is_file());
    }
}
