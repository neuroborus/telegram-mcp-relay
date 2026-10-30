use super::*;
use std::{
    ffi::OsString,
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::Read,
    os::unix::{
        ffi::OsStringExt,
        fs::{DirBuilderExt, OpenOptionsExt, symlink},
    },
    sync::Barrier,
    thread,
};

use rustix::fs::{CWD, FileType, RenameFlags, mkfifoat, mknodat, renameat_with};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("files-test-{:032x}", u128::from_ne_bytes(random)));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::create_dir(path.join("exports")).unwrap();
        Self(path)
    }

    fn root(&self) -> PathBuf {
        self.0.join("exports")
    }

    fn settings(&self, mb: u32) -> Files {
        Files {
            allowed_roots: vec![self.root()],
            max_outbound_mb: mb,
        }
    }

    fn access(&self) -> OutboundFiles {
        OutboundFiles::new(&self.settings(1)).unwrap()
    }

    fn sparse(&self, bytes: u64) -> PathBuf {
        let path = self.root().join("report.bin");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap()
            .set_len(bytes)
            .unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn read(opened: ValidatedFile) -> String {
    let (mut file, _, _) = opened.into_parts();
    let mut body = String::new();
    file.read_to_string(&mut body).unwrap();
    body
}

#[test]
fn regular_empty_and_nested_files_and_multiple_roots() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root().join("nested")).unwrap();
    let path = fixture.root().join("nested/report.md");
    fs::write(&path, "synthetic report").unwrap();
    let other = fixture.0.join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("empty"), "").unwrap();
    let mut settings = fixture.settings(1);
    settings.allowed_roots.push(other.clone());
    let access = OutboundFiles::new(&settings).unwrap();
    let opened = access.open(&path).unwrap();
    assert_eq!(opened.filename(), "report.md");
    assert_eq!(opened.size_bytes(), 16);
    assert_eq!(read(opened), "synthetic report");
    let empty = access.open(&other.join("empty")).unwrap();
    assert_eq!(empty.size_bytes(), 0);
    assert_eq!(read(empty), "");
}

#[test]
fn configured_and_cloud_decimal_limits_are_inclusive() {
    let fixture = Fixture::new();
    let path = fixture.sparse(1_000_000);
    let access = fixture.access();
    assert_eq!(access.open(&path).unwrap().size_bytes(), 1_000_000);
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(1_000_001).unwrap();
    assert_eq!(
        access.open(&path).unwrap_err(),
        AppError::FileTooLarge {
            limit_bytes: 1_000_000,
            actual_bytes: 1_000_001
        }
    );
    let cloud = OutboundFiles::new(&fixture.settings(50)).unwrap();
    file.set_len(50_000_000).unwrap();
    assert_eq!(cloud.open(&path).unwrap().size_bytes(), 50_000_000);
    file.set_len(50_000_001).unwrap();
    assert_eq!(
        cloud.open(&path).unwrap_err(),
        AppError::FileTooLarge {
            limit_bytes: 50_000_000,
            actual_bytes: 50_000_001
        }
    );
    for invalid in [0, 51, u32::MAX] {
        assert_eq!(
            OutboundFiles::new(&fixture.settings(invalid)).unwrap_err(),
            AppError::InvalidConfig("files.max_outbound_mb")
        );
    }
}

#[test]
fn invalid_paths_missing_files_and_prefix_collisions_fail_without_leaking_paths() {
    let fixture = Fixture::new();
    let sibling = fixture.0.join("exports-other");
    fs::create_dir(&sibling).unwrap();
    fs::write(sibling.join("private"), "outside").unwrap();
    let access = fixture.access();
    let invalid = [
        PathBuf::from("report.txt"),
        fixture.root().join("../exports-other/private"),
        fixture.root().join("nested/../../exports-other/private"),
        fixture.root().join("missing"),
        fixture.root().join("nul\0private"),
        fixture.root(),
        sibling.join("private"),
    ];
    for path in invalid {
        let error = access.open(&path).unwrap_err();
        assert_eq!(error, AppError::FileUnavailable);
        assert_eq!(error.code(), "FILE_UNAVAILABLE");
        assert!(!format!("{error:?} {error}").contains(fixture.0.to_str().unwrap()));
    }
    assert_eq!(fs::read(sibling.join("private")).unwrap(), b"outside");
}

#[test]
fn symlinks_at_roots_ancestors_intermediate_and_leaf_components_fail_closed() {
    let fixture = Fixture::new();
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("private"), "outside").unwrap();
    let regular = fixture.root().join("report");
    fs::write(&regular, "inside").unwrap();
    symlink(&outside, fixture.root().join("escape")).unwrap();
    symlink(&regular, fixture.root().join("internal-link")).unwrap();
    symlink(
        outside.join("private"),
        fixture.root().join("external-link"),
    )
    .unwrap();
    symlink("/proc/self/fd/0", fixture.root().join("magic-link")).unwrap();
    let access = fixture.access();
    for relative in [
        "escape/private",
        "internal-link",
        "external-link",
        "magic-link",
    ] {
        assert_eq!(
            access.open(&fixture.root().join(relative)).unwrap_err(),
            AppError::FileUnavailable
        );
    }
    let alias = fixture.0.join("alias");
    symlink(fixture.root(), &alias).unwrap();
    for root in [alias, fixture.root().join("escape")] {
        let mut settings = fixture.settings(1);
        settings.allowed_roots = vec![root];
        assert_eq!(
            OutboundFiles::new(&settings).unwrap_err(),
            AppError::FileUnavailable
        );
    }
    let parent_alias = fixture.0.join("parent-alias");
    symlink(&fixture.0, &parent_alias).unwrap();
    let mut settings = fixture.settings(1);
    settings.allowed_roots = vec![parent_alias.join("exports")];
    assert_eq!(
        OutboundFiles::new(&settings).unwrap_err(),
        AppError::FileUnavailable
    );
}

#[test]
fn directories_fifos_sockets_and_unreadable_files_fail_without_blocking() {
    let fixture = Fixture::new();
    let access = fixture.access();
    fs::create_dir(fixture.root().join("directory")).unwrap();
    mkfifoat(CWD, fixture.root().join("pipe"), Mode::RUSR | Mode::WUSR).unwrap();
    // An inert socket node avoids sun_path's short pathname limit under long TMPDIRs.
    mknodat(
        CWD,
        fixture.root().join("socket"),
        FileType::Socket,
        Mode::RUSR | Mode::WUSR,
        0,
    )
    .unwrap();
    let locked = fixture.root().join("locked");
    fs::write(&locked, "unreadable").unwrap();
    fs::set_permissions(&locked, Permissions::from_mode(0o000)).unwrap();
    for relative in ["directory", "pipe", "socket", "locked"] {
        assert_eq!(
            access.open(&fixture.root().join(relative)).unwrap_err(),
            AppError::FileUnavailable
        );
    }
    fs::set_permissions(&locked, Permissions::from_mode(0o400)).unwrap();
    assert_eq!(read(access.open(&locked).unwrap()), "unreadable");
}

#[test]
fn replacement_of_root_and_leaf_does_not_redirect_the_open_handle() {
    let fixture = Fixture::new();
    let path = fixture.root().join("report");
    fs::write(&path, "original").unwrap();
    let access = fixture.access();
    let opened = access.open(&path).unwrap();
    fs::rename(&path, fixture.root().join("old-report")).unwrap();
    fs::write(&path, "replacement").unwrap();
    assert_eq!(read(opened), "original");
    let pinned = fixture.0.join("pinned");
    fs::rename(fixture.root(), &pinned).unwrap();
    fs::create_dir(fixture.root()).unwrap();
    fs::write(&path, "unapproved replacement root").unwrap();
    assert_eq!(read(access.open(&path).unwrap()), "replacement");
    fs::remove_file(&path).unwrap();
    symlink(fixture.0.join("outside"), &path).unwrap();
    assert_eq!(read(access.open(&path).unwrap()), "replacement");
}

#[test]
fn racing_directory_and_leaf_symlink_swaps_cannot_escape() {
    let fixture = Fixture::new();
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("report"), "outside").unwrap();
    let access = fixture.access();
    for directory in [false, true] {
        let live = fixture.root().join("live");
        let alternate = fixture.root().join("alternate");
        let target = if directory {
            fs::create_dir(&live).unwrap();
            fs::write(live.join("report"), "inside").unwrap();
            symlink(&outside, &alternate).unwrap();
            live.join("report")
        } else {
            fs::write(&live, "inside").unwrap();
            symlink(outside.join("report"), &alternate).unwrap();
            live.clone()
        };
        let barrier = Barrier::new(2);
        thread::scope(|scope| {
            scope.spawn(|| {
                barrier.wait();
                for _ in 0..256 {
                    renameat_with(CWD, &live, CWD, &alternate, RenameFlags::EXCHANGE).unwrap();
                }
            });
            barrier.wait();
            for _ in 0..256 {
                match access.open(&target) {
                    Ok(opened) => assert_eq!(read(opened), "inside"),
                    Err(error) => assert_eq!(error, AppError::FileUnavailable),
                }
            }
        });
        if directory {
            fs::remove_dir_all(&live).unwrap();
        } else {
            fs::remove_file(&live).unwrap();
        }
        fs::remove_file(&alternate).unwrap();
    }
}

#[test]
fn filenames_are_bounded_header_safe_basenames_and_debug_is_redacted() {
    let fixture = Fixture::new();
    let access = fixture.access();
    for name in [
        OsString::from("résumé\r\n\"\\;界.txt"),
        OsString::from("a".repeat(200)),
        OsString::from_vec(b"invalid-\xff.bin".to_vec()),
        OsString::from("..."),
    ] {
        let path = fixture.root().join(name);
        fs::write(&path, "synthetic").unwrap();
        let opened = access.open(&path).unwrap();
        assert!(!opened.filename().is_empty());
        assert!(opened.filename().len() <= 128);
        assert!(
            opened
                .filename()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        );
        assert_ne!(opened.filename(), ".");
        assert_ne!(opened.filename(), "..");
        assert!(!format!("{opened:?} {access:?}").contains(fixture.0.to_str().unwrap()));
        assert!(!format!("{opened:?}").contains(opened.filename()));
    }
}

#[test]
fn invalid_or_unavailable_root_configuration_fails_without_creating_paths() {
    let fixture = Fixture::new();
    for roots in [
        vec![],
        vec![fixture.root(); 65],
        vec![PathBuf::from("/")],
        vec![PathBuf::from("relative")],
        vec![fixture.root().join("..")],
    ] {
        let mut settings = fixture.settings(1);
        settings.allowed_roots = roots;
        assert_eq!(
            OutboundFiles::new(&settings).unwrap_err(),
            AppError::InvalidConfig("files.allowed_roots")
        );
    }
    let missing = fixture.0.join("missing");
    let mut settings = fixture.settings(1);
    settings.allowed_roots = vec![missing.clone()];
    assert_eq!(
        OutboundFiles::new(&settings).unwrap_err(),
        AppError::FileUnavailable
    );
    assert!(!missing.exists());
    let regular = fixture.sparse(0);
    settings.allowed_roots = vec![regular];
    assert_eq!(
        OutboundFiles::new(&settings).unwrap_err(),
        AppError::FileUnavailable
    );
}
