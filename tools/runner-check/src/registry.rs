//! Provision only locked crates.io sparse metadata and checksum-verified archives.

use std::{
    collections::BTreeSet,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use crate::{Result, checked_path, invalid, sha256::Sha256};

const REGISTRY: &str = "index.crates.io-1949cf8c6b5b557f";
const SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

#[derive(Debug)]
pub(crate) struct Package {
    archive: String,
    index: PathBuf,
    checksum: String,
}

/// Read the narrow generated Cargo.lock v4 package fields we need; Cargo itself performs
/// the complete locked/offline manifest and index resolution. Other sources fail closed.
pub(crate) fn packages(lock: &str) -> Result<Vec<Package>> {
    let mut sections = lock.split("[[package]]");
    if !sections
        .next()
        .unwrap_or_default()
        .lines()
        .any(|line| line == "version = 4")
    {
        return Err(invalid("expected Cargo.lock v4"));
    }
    let mut packages = Vec::new();
    for section in sections {
        let mut name = None;
        let mut version = None;
        let mut source = None;
        let mut checksum = None;
        for line in section.lines() {
            let Some((key, value)) = line.split_once(" = ") else {
                continue;
            };
            let field = match key {
                "name" => &mut name,
                "version" => &mut version,
                "source" => &mut source,
                "checksum" => &mut checksum,
                _ => continue,
            };
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .ok_or_else(|| invalid("invalid lock field"))?;
            if field.replace(value).is_some() {
                return Err(invalid("duplicate lock field"));
            }
        }
        let name = name.ok_or_else(|| invalid("missing package name"))?;
        let version = version.ok_or_else(|| invalid("missing package version"))?;
        for value in [name, version] {
            if !value.starts_with(|c: char| c.is_ascii_alphanumeric())
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.+".contains(&c))
            {
                return Err(invalid("invalid package identity"));
            }
        }
        let Some(source) = source else {
            if checksum.is_some() {
                return Err(invalid("checksum without registry source"));
            }
            continue;
        };
        if source != SOURCE {
            return Err(invalid("only crates.io registry packages are supported"));
        }
        let checksum = checksum.ok_or_else(|| invalid("missing archive checksum"))?;
        if checksum.len() != 64 || !checksum.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(invalid("invalid archive checksum"));
        }
        let lower = name.to_ascii_lowercase();
        let prefix = match lower.len() {
            1 => "1".into(),
            2 => "2".into(),
            3 => format!("3/{}", &lower[..1]),
            _ => format!("{}/{}", &lower[..2], &lower[2..4]),
        };
        packages.push(Package {
            archive: format!("{name}-{version}.crate"),
            index: Path::new(&prefix).join(lower),
            checksum: checksum.to_ascii_lowercase(),
        });
    }
    Ok(packages)
}

fn copy(source: &Path, home: &Path, relative: &Path, checksum: Option<&str>) -> Result<()> {
    let source = checked_path(source, relative, false)?;
    let destination = home.join("registry").join(relative);
    DirBuilder::new().recursive(true).mode(0o700).create(
        destination
            .parent()
            .ok_or_else(|| invalid("missing destination parent"))?,
    )?;
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&destination)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 32_768];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        if checksum.is_some() {
            hash.update(&buffer[..count]);
        }
    }
    if checksum.is_some_and(|expected| hash.finish() != expected) {
        drop(output);
        fs::remove_file(destination)?;
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "archive checksum mismatch",
        ));
    }
    Ok(())
}

pub(crate) fn provision(source: &Path, home: &Path, packages: &[Package]) -> Result<()> {
    // Do not traverse the source's src/, credentials, Cargo configuration or cache databases.
    // Only the exact sparse entries and archives named by the selected lockfile are inputs.
    if packages.is_empty() {
        return Ok(());
    }
    let index = Path::new("index").join(REGISTRY);
    copy(source, home, &index.join("config.json"), None)?;
    let mut copied = BTreeSet::new();
    for package in packages {
        if copied.insert(&package.index) {
            copy(
                source,
                home,
                &index.join(".cache").join(&package.index),
                None,
            )?;
        }
        copy(
            source,
            home,
            &Path::new("cache").join(REGISTRY).join(&package.archive),
            Some(&package.checksum),
        )?;
    }
    Ok(())
}
