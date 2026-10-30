use super::*;
use std::{
    collections::BTreeMap,
    os::unix::fs::{PermissionsExt, symlink},
};

const REGISTRY: &str = "index.crates.io-1949cf8c6b5b557f";
const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn options(mode: &str) -> Options {
    Options::parse(&[
        mode.into(),
        "--toolchain-bin".into(),
        "/exposed/bin".into(),
        "--registry-source".into(),
        "approved/registry".into(),
    ])
    .unwrap()
}

fn temporary() -> PrivateDirectory {
    PrivateDirectory::new(&env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn write(root: &Path, path: &str, value: &[u8]) -> PathBuf {
    let path = root.join(path);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path.parent().unwrap())
        .unwrap();
    fs::write(&path, value).unwrap();
    path
}

fn lock(version: &str, checksum: &str) -> String {
    format!(
        "version = 4\n\n[[package]]\nname = \"demo\"\nversion = \"{version}\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{checksum}\"\n"
    )
}

fn registry_source() -> PrivateDirectory {
    let source = temporary();
    write(&source.0, &format!("index/{REGISTRY}/config.json"), b"{}");
    write(
        &source.0,
        &format!("index/{REGISTRY}/.cache/de/mo/demo"),
        b"synthetic sparse metadata",
    );
    write(
        &source.0,
        &format!("cache/{REGISTRY}/demo-1.0.0.crate"),
        b"abc",
    );
    source
}

#[test]
fn cli_is_closed_and_paths_cannot_escape() {
    for mode in [
        "clippy-workspace",
        "doc-workspace",
        "test-workspace",
        "clippy-helper",
        "doc-helper",
        "test-helper",
    ] {
        assert_eq!(Mode::parse(OsStr::new(mode)).unwrap(), options(mode).mode);
    }
    for mode in [
        "test",
        "release",
        "shell",
        "fetch",
        "",
        "test-workspace -- --ignored",
    ] {
        assert!(Mode::parse(OsStr::new(mode)).is_err());
    }
    assert!(Options::parse(&[]).is_err());
    let valid: Vec<OsString> = [
        "test-helper",
        "--toolchain-bin",
        "/actual/bin",
        "--registry-source",
        "approved/registry",
    ]
    .map(Into::into)
    .into();
    for path in [
        "",
        "/outside",
        "../outside",
        "approved/../../outside",
        "./registry",
    ] {
        let mut args = valid.clone();
        args[4] = path.into();
        assert!(Options::parse(&args).is_err());
    }
    for path in ["relative/bin", "/actual/../other/bin"] {
        let mut args = valid.clone();
        args[2] = path.into();
        assert!(Options::parse(&args).is_err());
    }
    let mut args = valid.clone();
    args.push("--network".into());
    assert!(Options::parse(&args).is_err());
    let mut args = valid;
    args[1] = "--command".into();
    assert!(Options::parse(&args).is_err());
}

#[test]
fn modes_keep_complete_flags_and_helper_is_a_separate_workspace() {
    for (mode, subcommand, helper) in [
        ("clippy-workspace", "clippy", false),
        ("doc-workspace", "doc", false),
        ("test-workspace", "test", false),
        ("clippy-helper", "clippy", true),
        ("doc-helper", "doc", true),
        ("test-helper", "test", true),
    ] {
        let options = options(mode);
        let command = command(
            &options,
            Path::new("/checkout"),
            Path::new("/scratch"),
            Path::new("/cache/home"),
        )
        .unwrap();
        assert_eq!(command.get_program(), "/exposed/bin/cargo");
        assert_eq!(command.get_current_dir(), Some(Path::new("/checkout")));
        let mut expected = vec![subcommand];
        if helper {
            expected.extend(["--manifest-path", "tools/runner-check/Cargo.toml"]);
        }
        expected.extend(["--locked", "--workspace"]);
        match subcommand {
            "clippy" => expected.push("--all-targets"),
            "doc" => expected.extend(["--no-deps", "--document-private-items"]),
            _ => (),
        }
        expected.extend(["--offline", "--target-dir", "/scratch/target"]);
        if subcommand == "clippy" {
            expected.extend(["--", "-D", "warnings"]);
        }
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            expected.iter().map(OsStr::new).collect::<Vec<_>>()
        );
        let env: BTreeMap<_, _> = command
            .get_envs()
            .map(|(key, value)| (key.to_str().unwrap(), value.unwrap().to_str().unwrap()))
            .collect();
        assert_eq!(env["HOME"], "/nonexistent");
        assert_eq!(env["PATH"], "/exposed/bin:/usr/bin:/bin");
        assert_eq!(env["RUSTC"], "/exposed/bin/rustc");
        assert_eq!(env["RUSTDOC"], "/exposed/bin/rustdoc");
        assert_eq!(env["CARGO_HOME"], "/cache/home");
        assert_eq!(env["TMPDIR"], "/scratch/tmp");
        assert_eq!(env["CARGO_NET_OFFLINE"], "true");
        assert_eq!(
            env.get("RUSTDOCFLAGS"),
            (subcommand == "doc").then_some(&"-D warnings")
        );
        assert!(!env.contains_key("RUSTC_WRAPPER"));
    }
}

#[test]
fn preparation_failure_propagates_and_cleans_private_state_without_running_a_check() {
    let checkout = temporary();
    let source = registry_source();
    let scratch = temporary();
    let cache = temporary();
    fs::create_dir(checkout.0.join("approved")).unwrap();
    fs::rename(&source.0, checkout.0.join("approved/registry")).unwrap();
    write(&checkout.0, "Cargo.lock", lock("1.0.0", ABC).as_bytes());
    write(
        &checkout.0,
        &format!("approved/registry/cache/{REGISTRY}/demo-1.0.0.crate"),
        b"corrupt",
    );
    let result = prepare(
        &options("test-workspace"),
        &checkout.0,
        &scratch.0,
        &cache.0,
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("corrupt archive accepted"),
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&cache.0).unwrap().count(), 0);
    assert!(!checkout.0.join("target").exists());
}

#[test]
fn helper_preparation_uses_its_own_lock_without_product_dependencies() {
    let checkout = temporary();
    let scratch = temporary();
    let cache = temporary();
    write(
        &checkout.0,
        "approved/registry/src/ignored",
        b"must not copy",
    );
    write(
        &checkout.0,
        "Cargo.lock",
        b"invalid product lock; must not read",
    );
    write(
        &checkout.0,
        "tools/runner-check/Cargo.lock",
        b"version = 4\n[[package]]\nname = \"runner-check\"\nversion = \"0.1.0\"\n",
    );
    let prepared = prepare(&options("test-helper"), &checkout.0, &scratch.0, &cache.0).unwrap();
    assert!(
        prepared
            .command
            .get_args()
            .any(|arg| arg == "tools/runner-check/Cargo.toml")
    );
    assert!(!prepared._home.0.join("registry").exists());
    assert!(prepared._scratch.0.join("tmp").is_dir());
    drop(prepared);
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&cache.0).unwrap().count(), 0);
}

#[test]
fn private_directories_are_exclusive_restrictive_and_cleaned() {
    let parent = temporary();
    let first = PrivateDirectory::new(&parent.0).unwrap();
    let second = PrivateDirectory::new(&parent.0).unwrap();
    assert_ne!(first.0, second.0);
    assert_eq!(
        fs::metadata(&first.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let path = first.0.clone();
    write(&path, "nested/file", b"fixture");
    drop(first);
    assert!(!path.exists());
    assert!(second.0.exists());
}

#[test]
fn confinement_rejects_leaf_and_ancestor_symlinks_and_hard_links() {
    let root = temporary();
    let outside = temporary();
    write(&outside.0, "archive", b"synthetic");
    symlink(&outside.0, root.0.join("linked")).unwrap();
    symlink(outside.0.join("archive"), root.0.join("leaf")).unwrap();
    fs::hard_link(outside.0.join("archive"), root.0.join("hard")).unwrap();
    for path in ["linked/archive", "leaf", "hard", "../archive", "/archive"] {
        assert!(checked_path(&root.0, Path::new(path), false).is_err());
    }
    assert!(checked_path(&root.0, Path::new("linked"), true).is_err());
}

#[test]
fn provisioning_copies_only_locked_sparse_entries_and_verified_archives() {
    let source = registry_source();
    let home = temporary();
    for path in [
        "credentials.toml",
        "config.toml",
        ".global-cache",
        "src/host-source/lib.rs",
        "index/unapproved/config.json",
        "cache/unapproved/other.crate",
    ] {
        write(&source.0, path, b"must not copy");
    }
    let second = lock("2.0.0", ABC);
    write(
        &source.0,
        &format!("cache/{REGISTRY}/demo-2.0.0.crate"),
        b"abc",
    );
    let lock = lock("1.0.0", ABC) + second.strip_prefix("version = 4\n").unwrap();
    registry::provision(&source.0, &home.0, &registry::packages(&lock).unwrap()).unwrap();
    assert_eq!(
        fs::read(
            home.0
                .join(format!("registry/cache/{REGISTRY}/demo-1.0.0.crate"))
        )
        .unwrap(),
        b"abc"
    );
    assert_eq!(
        fs::read(
            home.0
                .join(format!("registry/cache/{REGISTRY}/demo-2.0.0.crate"))
        )
        .unwrap(),
        b"abc"
    );
    assert_eq!(fs::read_dir(home.0.join("registry")).unwrap().count(), 2);
    for path in [
        "credentials.toml",
        "config.toml",
        ".global-cache",
        "registry/src",
        "registry/index/unapproved",
        "registry/cache/unapproved",
    ] {
        assert!(!home.0.join(path).exists());
    }
    let archive = home
        .0
        .join(format!("registry/cache/{REGISTRY}/demo-1.0.0.crate"));
    assert_eq!(
        fs::metadata(archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn missing_corrupt_and_linked_registry_inputs_fail_closed() {
    for input in [
        "corrupt",
        "missing",
        "symlink",
        "index-symlink",
        "ancestor-symlink",
    ] {
        let source = registry_source();
        let home = temporary();
        let archive = source.0.join(format!("cache/{REGISTRY}/demo-1.0.0.crate"));
        match input {
            "corrupt" => fs::write(&archive, b"abd").unwrap(),
            "missing" => fs::remove_file(&archive).unwrap(),
            "symlink" => {
                fs::remove_file(&archive).unwrap();
                symlink("/outside/archive", &archive).unwrap();
            }
            "index-symlink" => {
                let index = source.0.join(format!("index/{REGISTRY}/.cache/de/mo/demo"));
                fs::remove_file(&index).unwrap();
                symlink("/outside/index", index).unwrap();
            }
            _ => {
                fs::rename(source.0.join("cache"), source.0.join("moved")).unwrap();
                symlink(source.0.join("moved"), source.0.join("cache")).unwrap();
            }
        }
        assert!(
            registry::provision(
                &source.0,
                &home.0,
                &registry::packages(&lock("1.0.0", ABC)).unwrap()
            )
            .is_err(),
            "accepted {input}"
        );
    }
}

#[test]
fn lock_sources_and_checksums_cannot_redirect_provisioning() {
    let valid = lock("1.0.0", ABC);
    for text in [
        valid.replace("version = 4", "version = 3"),
        valid.replace(
            "registry+https://github.com/rust-lang/crates.io-index",
            "git+https://example.invalid/repo",
        ),
        valid.replace(ABC, "bad"),
        valid.replace("demo", "../../secret"),
        valid.replace("version = \"1.0.0\"", "version = \"../outside\""),
        valid.replace(&format!("checksum = \"{ABC}\""), ""),
        valid.replace("name = \"demo\"", "name = \"demo\"\nname = \"other\""),
    ] {
        assert!(registry::packages(&text).is_err());
    }
    assert!(
        registry::packages(
            "version = 4\n[[package]]\nname = \"runner-check\"\nversion = \"0.1.0\"\n"
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn sha256_matches_standard_vectors_across_block_and_chunk_boundaries() {
    let cases = [
        (
            "",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        ("abc", ABC),
        (
            "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
    ];
    for (input, expected) in cases {
        for chunk_size in [1, 7, 64, 4096] {
            let mut hash = sha256::Sha256::new();
            for chunk in input.as_bytes().chunks(chunk_size) {
                hash.update(chunk);
            }
            assert_eq!(hash.finish(), expected);
        }
    }
    let mut hash = sha256::Sha256::new();
    for _ in 0..1000 {
        hash.update(&[b'a'; 1000]);
    }
    assert_eq!(
        hash.finish(),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}
