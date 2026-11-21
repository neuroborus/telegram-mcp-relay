use super::*;
use std::{
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink},
};

const BASE: &str = "[telegram]\nchat_id = 101\nallowed_chat_ids = [101]\nallowed_user_ids = [202]\n[files]\nallowed_roots = ['~/exports']\n";
const BOT: &str = "123456:synthetic_token";

fn parse(extra: &str) -> Result<Config, AppError> {
    let input = format!("{BASE}{extra}");
    Config::parse(&input, Some(Path::new("/home/synthetic")))
}

fn parse_telegram(settings: &str, extra: &str) -> Result<Config, AppError> {
    let input = BASE.replacen("[telegram]\n", &format!("[telegram]\n{settings}\n"), 1);
    Config::parse(
        &format!("{input}{extra}"),
        Some(Path::new("/home/synthetic")),
    )
}

#[test]
fn defaults_and_example_are_safe_and_independent() {
    let config = parse("").unwrap();
    assert!(config.server.listen.ip().is_loopback());
    assert_eq!(config.telegram.bot_api, BotApi::Cloud);
    assert_eq!(config.telegram.local_bot_api_address().unwrap(), None);
    assert!(config.delivery.wait_fallback_enabled && config.activity.typing_enabled);
    assert_eq!(config.delivery.wait_timeout_seconds, 600);
    assert!(!config.history.enabled);
    assert!(!config.downloads.enabled && config.downloads.roots.is_empty());
    assert_eq!(config.activity.typing_max_duration_seconds, 600);
    let storage = &config.storage;
    assert_eq!(storage.session_ttl_days, 30);
    assert_eq!(storage.route_ttl_days, 30);
    assert_eq!(storage.reply_ttl_days, 30);
    assert_eq!(
        config.files.allowed_roots,
        [PathBuf::from("/home/synthetic/exports")]
    );
    Config::parse(
        include_str!("../../config/relay.example.toml"),
        Some(Path::new("/home/synthetic")),
    )
    .unwrap();
    let config = parse("[delivery]\nwait_fallback_enabled=false\nwait_timeout_seconds=900\n[activity]\ntyping_max_duration_seconds=8\n").unwrap();
    assert_eq!(config.activity.typing_refresh_seconds, 4);
    assert_eq!(config.delivery.wait_timeout_seconds, 900);
}

#[test]
fn local_bot_api_accepts_explicit_loopback_and_keeps_cloud_defaults() {
    let root = "local_bot_api_files_root = '~/upstream'";
    let settings = |url: &str| format!("bot_api = 'local'\n{root}\nlocal_bot_api_url = '{url}'");
    for address in ["127.0.0.2:8081", "[::1]:80"] {
        let config = parse_telegram(&settings(&format!("http://{address}")), "").unwrap();
        assert_eq!(config.telegram.bot_api, BotApi::Local);
        assert_eq!(
            config.telegram.local_bot_api_address().unwrap(),
            Some(address.parse().unwrap())
        );
        assert_eq!(
            config.telegram.local_bot_api_files_root.as_deref(),
            Some(Path::new("/home/synthetic/upstream"))
        );
        assert_eq!(config.downloads.max_file_bytes, CLOUD_DOWNLOAD_BYTES);
        assert_eq!(config.files.max_outbound_mb, 50);
    }
    let local = settings("http://127.0.0.1:8081");
    let mut config = parse_telegram(&local, "").unwrap();
    config.files.max_outbound_mb = 51;
    assert_eq!(
        config.validate(None).unwrap_err(),
        AppError::InvalidConfig("files.max_outbound_mb")
    );
}

#[test]
fn download_limits_follow_the_selected_backend() {
    for (settings, file, total, timeout) in [
        ("", 20_000_000_u64, 1 << 30, 600),
        (
            "bot_api='local'\nlocal_bot_api_url='http://127.0.0.1:8081'\nlocal_bot_api_files_root='~/upstream'",
            4 << 30,
            64 << 30,
            3_600,
        ),
    ] {
        let parse = |file, total, timeout| {
            parse_telegram(
                settings,
                &format!(
                    "[downloads]\nenabled=true\nroots=['~/downloads']\nmax_file_bytes={file}\nmax_total_bytes={total}\ntimeout_seconds={timeout}"
                ),
            )
        };
        assert!(parse(file, total, timeout).unwrap().downloads.enabled);
        for (result, field) in [
            (parse(file + 1, total, timeout), "downloads.max_file_bytes"),
            (parse(file, total + 1, timeout), "downloads.max_total_bytes"),
            (parse(file, file - 1, timeout), "downloads.max_total_bytes"),
            (parse(file, total, timeout + 1), "downloads.timeout_seconds"),
        ] {
            assert_eq!(result.unwrap_err(), AppError::InvalidConfig(field));
        }
    }
}

#[test]
fn local_bot_api_rejects_anything_beyond_an_explicit_http_loopback_authority() {
    for url in [
        "http://192.0.2.1:8081",
        "http://localhost:8081",
        "http://127.1:8081",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "https://127.0.0.1:8081",
        "http://user:secret@127.0.0.1:8081",
        "http://127.0.0.1:8081/",
        "http://127.0.0.1:8081/prefix/..",
        "http://127.0.0.1:8081?",
        "http://127.0.0.1:8081#",
        "http://[::1%0]:8081",
    ] {
        let settings = format!(
            "bot_api='local'\nlocal_bot_api_url='{url}'\nlocal_bot_api_files_root='/upstream'"
        );
        assert_eq!(
            parse_telegram(&settings, "").unwrap_err(),
            AppError::InvalidConfig("telegram.local_bot_api_url"),
            "{url}"
        );
    }
    for (settings, field) in [
        (
            "bot_api='local'\nlocal_bot_api_files_root='/upstream'",
            "telegram.local_bot_api_url",
        ),
        (
            "bot_api='local'\nlocal_bot_api_url='http://127.0.0.1:8081'",
            "telegram.local_bot_api_files_root",
        ),
        (
            "bot_api='cloud'\nlocal_bot_api_url='http://127.0.0.1:8081'",
            "telegram.local_bot_api_url",
        ),
        (
            "local_bot_api_files_root='/upstream'",
            "telegram.local_bot_api_files_root",
        ),
        ("bot_api='auto'", "TOML document"),
    ] {
        assert_eq!(
            parse_telegram(settings, "").unwrap_err(),
            AppError::InvalidConfig(field)
        );
    }
}

#[test]
fn local_bot_api_root_must_be_absolute_and_separate_from_relay_paths() {
    for (root, extra, telegram_extra) in [
        ("relative", "", ""),
        ("/", "", ""),
        ("/private/../upstream", "", ""),
        ("~/exports", "", ""),
        ("~/exports/upstream", "", ""),
        (
            "~/upstream",
            "[downloads]\nroots=['~/upstream/downloads']",
            "",
        ),
        (
            "~/upstream",
            "[storage]\npath='~/upstream/relay.sqlite'",
            "",
        ),
        (
            "~/upstream",
            "[server]\nauth_token_file='~/upstream/auth'",
            "",
        ),
        ("~/upstream", "", "bot_token_file='~/upstream/bot'"),
    ] {
        let settings = format!(
            "bot_api='local'\nlocal_bot_api_url='http://127.0.0.1:8081'\nlocal_bot_api_files_root='{root}'\n{telegram_extra}"
        );
        assert_eq!(
            parse_telegram(&settings, extra).unwrap_err(),
            AppError::InvalidConfig("telegram.local_bot_api_files_root"),
            "{root} {extra} {telegram_extra}"
        );
    }
}

#[test]
fn rejects_invalid_settings_before_resolving_secrets() {
    assert!(Config::parse("", None).is_err());
    for extra in [
        "[server]\nlisten='0.0.0.0:8765'",
        "[server]\nlisten='[::]:8765'",
        "[server]\nlisten='127.0.0.1:0'",
        "[server]\ncreate_auth_token_file=true",
        "[server]\nauth_token_env='TOKEN'\nauth_token_file='/private/token'",
        "[server]\nauth_token_env='BAD-NAME'",
        "[server]\nauth_token_env=''",
        "[delivery]\nwait_timeout_seconds=0",
        "[delivery]\nwait_timeout_seconds=86401",
        "[delivery]\nclaim_lease_seconds=601",
        "[delivery]\nprocessing_start_grace_seconds=0",
        "[activity]\ntyping_enabled=false\ntyping_max_duration_seconds=601",
        "[activity]\ntyping_refresh_seconds=6",
        "[activity]\ntyping_max_duration_seconds=2",
        "[storage]\nreply_ttl_days=0",
        "[storage]\nroute_ttl_days=3651",
        "[storage]\nmax_database_bytes=1024",
        "[storage]\ncleanup_interval_hours=0",
        "[history]\nenabled=true",
        "[history]\nretention_days=0",
        "[history]\nmax_records=100001",
        "[history]\nmax_attachments=0",
        "[history]\nmax_record_bytes=8388609",
        "[history]\nmax_body_bytes=1",
        "[history]\nmax_metadata_bytes=0",
        "[history]\npage_limit=101",
        "[history]\nresponse_bytes=0",
        "[history]\nquery_bytes=4097",
        "[history]\nscan_records=1",
        "[downloads]\nenabled=true",
        "[downloads]\nmax_file_bytes=0",
        "[downloads]\nmax_file_bytes=20000001",
        "[downloads]\nmax_total_bytes=1",
        "[downloads]\nmax_files=0",
        "[downloads]\nmax_files=1025",
        "[downloads]\nconcurrent=0",
        "[downloads]\nconcurrent=65",
        "[downloads]\ntimeout_seconds=601",
        "[downloads]\nretention_seconds=0",
        "[downloads]\ncleanup_interval_seconds=0",
        "[downloads]\nmin_free_bytes=0",
        "[downloads]\nroots=['/']",
        "[downloads]\nroots=['relative']",
        "[downloads]\nroots=['/private/../downloads']",
        "[downloads]\nroots=['~/exports/downloads']",
        "[downloads]\nroots=['/home/synthetic']",
        "[downloads]\nroots=['/private/downloads','/private/downloads/nested']",
        "[limits]\nconcurrent_requests=0",
        "[limits]\nwaiters=33",
        "[limits]\nreply_bytes=16777217",
        "[limits]\nupload_buffer_bytes=65537",
        "[limits]\ninbox_bytes=67108864",
        "[limits]\nrequest_bytes=-1",
        "[limits]\nrequest_bytes=4294967296",
        "[unknown]\nenabled=true",
        "[server]\noutbound_scalar_limit=1001",
        "[telegram.extra]\napi_url='http://fake'",
    ] {
        assert!(parse(extra).is_err(), "accepted invalid settings: {extra}");
    }
    for (from, to) in [
        ("chat_id = 101", "chat_id = 999"),
        ("[101]", "[]"),
        ("[202]", "[0]"),
        ("[202]", "[4503599627370496]"),
        ("'~/exports'", "'/'"),
        ("'~/exports'", "'relative/path'"),
        ("'~/exports'", "'/private/../exports'"),
        ("'~/exports'", "'~someone/exports'"),
    ] {
        assert!(
            Config::parse(&BASE.replace(from, to), Some(Path::new("/home/synthetic"))).is_err()
        );
    }
    assert!(Config::parse(BASE, None).is_err());
    assert!(Config::parse(&" ".repeat(CONFIG_BYTES + 1), None).is_err());
    assert!(
        Config::parse(
            &BASE.replace("allowed_roots =", "max_outbound_mb = 51\nallowed_roots ="),
            Some(Path::new("/home/synthetic"))
        )
        .is_err()
    );
}

#[test]
fn download_paths_are_explicit_separate_and_cannot_export_storage_or_secrets() {
    let config = parse("[downloads]\nenabled=true\nroots=['~/downloads']").unwrap();
    assert_eq!(
        config.downloads.roots,
        [PathBuf::from("/home/synthetic/downloads")]
    );
    assert_eq!(config.downloads.max_file_bytes, 20_000_000);
    assert_eq!(config.downloads.retention_seconds, 3_600);
    assert!(parse("[storage]\npath='/private/downloads/relay.sqlite'\n[downloads]\nroots=['/private/downloads']").is_err());
    let bot = BASE.replace(
        "chat_id = 101",
        "bot_token_file='/private/downloads/token'\nchat_id = 101",
    );
    assert!(
        Config::parse(
            &format!("{bot}[downloads]\nroots=['/private/downloads']"),
            Some(Path::new("/home/synthetic"))
        )
        .is_err()
    );
    assert!(parse("[server]\nauth_token_file='/private/downloads/token'\n[downloads]\nroots=['/private/downloads']").is_err());
}

#[test]
fn history_opt_in_requires_authentication_and_fits_the_separate_storage_budget() {
    let config = parse("[server]\nauth_token_env='AUTH'\n[history]\nenabled=true").unwrap();
    assert!(config.history.enabled);
    assert!(
        parse("[server]\nauth_token_env='AUTH'\n[history]\nenabled=true\nmax_body_bytes=67108864")
            .is_err()
    );
    // No archive budget is reserved when capture is disabled.
    assert!(parse("[storage]\nmax_database_bytes=34603008").is_ok());
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).unwrap();
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("config-test-{:016x}", u64::from_ne_bytes(bytes)));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, value: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap()
            .write_all(value)
            .unwrap();
        path
    }
    fn config(&self, extra: &str) -> Config {
        let input = BASE.replace(
            "chat_id = 101",
            &format!("bot_token_file = {:?}\nchat_id = 101", self.0.join("bot")),
        );
        Config::parse(&format!("{input}{extra}"), Some(&self.0)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn local_bot_api_config(upstream: &Path, exports: &Path, database: &Path) -> String {
    format!(
        "[telegram]\n\
         chat_id = 101\nallowed_chat_ids = [101]\nallowed_user_ids = [202]\n\
         bot_api = 'local'\nlocal_bot_api_url = 'http://127.0.0.1:8081'\n\
         local_bot_api_files_root = {upstream:?}\n\
         [files]\nallowed_roots = [{exports:?}]\n\
         [storage]\npath = {database:?}\n"
    )
}

#[test]
fn loading_local_bot_api_config_rejects_overlap_through_a_symlinked_ancestor() {
    let fixture = Fixture::new();
    let upstream = fixture.0.join("upstream");
    fs::create_dir_all(upstream.join("private")).unwrap();
    let alias = fixture.0.join("alias");
    symlink(&upstream, &alias).unwrap();
    let exports = fixture.0.join("exports");
    let input = |database: &Path| local_bot_api_config(&upstream, &exports, database);
    let separate_db = fixture.0.join("relay.sqlite");
    let protected = alias.join("private");
    let token = protected.join("token");
    for (name, config) in [
        ("overlap-db.toml", input(&protected.join("relay.sqlite"))),
        (
            "overlap-token.toml",
            format!(
                "{}[server]\nauth_token_file = {token:?}\n",
                input(&separate_db)
            ),
        ),
    ] {
        let config_path = fixture.write(name, config.as_bytes());
        assert_eq!(
            Config::load(&config_path).unwrap_err(),
            AppError::InvalidConfig(LOCAL_BOT_API_FILES_ROOT)
        );
    }
    let config_path = fixture.write("separate.toml", input(&separate_db).as_bytes());
    assert!(Config::load(&config_path).is_ok());
    assert!(!separate_db.exists());
    assert!(!token.exists());
}

#[test]
fn local_bot_api_rejects_a_dangling_upstream_ancestor() {
    let fixture = Fixture::new();
    let exports = fixture.0.join("exports");
    fs::create_dir(&exports).unwrap();
    let alias = fixture.0.join("alias");
    symlink(exports.join("missing"), &alias).unwrap();
    let mountinfo = b"1 0 8:1 / / rw - ext4 /dev/synthetic rw\n";
    let database = fixture.0.join("relay.sqlite");
    for upstream in [alias.join("upstream"), alias.join(".")] {
        let input = local_bot_api_config(&upstream, &exports, &database);
        let config = Config::parse(&input, None).unwrap();
        assert_eq!(
            config.validate_local_path_aliases(&upstream, mountinfo),
            Err(AppError::InvalidConfig(LOCAL_BOT_API_FILES_ROOT))
        );
    }
}

#[test]
fn loading_local_bot_api_config_rejects_a_file_as_the_working_directory() {
    let fixture = Fixture::new();
    let upstream = fixture.0.join("upstream");
    let database = fixture.write("relay.sqlite", b"synthetic");
    let input = local_bot_api_config(&upstream, &fixture.0.join("exports"), &database);
    let path = fixture.write("local.toml", input.as_bytes());
    assert!(Config::load(&path).is_ok());
    fixture.write("upstream", b"synthetic");
    assert_eq!(
        Config::load(&path).unwrap_err(),
        AppError::InvalidConfig(LOCAL_BOT_API_FILES_ROOT)
    );
}

#[test]
fn filesystem_prefix_overlap_detects_bind_mount_aliases() {
    // Model two disjoint mount paths for the same directory without creating a mount.
    let prefix = |device, inode, remainder: &'static str| ExistingPathPrefix {
        device,
        inode,
        remainder: Path::new(remainder),
    };
    let upstream = prefix(7, 42, "");
    assert!(upstream.overlaps(&prefix(7, 42, "")));
    assert!(upstream.overlaps(&prefix(7, 42, "private/token")));
    assert!(!upstream.overlaps(&prefix(8, 42, "")));
    assert!(!upstream.overlaps(&prefix(7, 43, "")));
    assert!(prefix(7, 42, "private").overlaps(&prefix(7, 42, "private/token")));
    assert!(!prefix(7, 42, "private").overlaps(&prefix(7, 42, "exports")));
}

fn mount_field(path: &Path) -> String {
    path.to_str()
        .unwrap()
        .replace('\\', "\\134")
        .replace(' ', "\\040")
        .replace('\t', "\\011")
        .replace('\n', "\\012")
}

#[test]
fn local_bot_api_rejects_namespace_mounts_that_cover_configured_paths() {
    let fixture = Fixture::new();
    let upstream = fixture.0.join("upstream");
    let exports = fixture.0.join("exports");
    let database = fixture.0.join("relay.sqlite");
    fs::create_dir(&upstream).unwrap();
    fs::create_dir(&exports).unwrap();
    let input = local_bot_api_config(&upstream, &exports, &database);
    let config = Config::parse(&input, None).unwrap();
    let validate = |point: &Path| {
        let mountinfo = format!(
            "1 0 8:1 / / rw - ext4 /dev/synthetic rw\n\
             2 1 0:4 mnt:[4026537000] {} rw - nsfs nsfs rw\n",
            mount_field(point)
        );
        config.validate_local_path_aliases(&upstream, mountinfo.as_bytes())
    };

    for point in [
        fixture.0.join("unrelated"),
        upstream.join("namespace"),
        exports.join("namespace"),
    ] {
        assert_eq!(validate(&point), Ok(()), "{point:?}");
    }
    for point in [&upstream, &exports, &database, &fixture.0] {
        assert_eq!(
            validate(point),
            Err(AppError::InvalidConfig(LOCAL_BOT_API_FILES_ROOT)),
            "{point:?}"
        );
    }
}

#[test]
fn local_bot_api_rejects_a_nested_bind_mount_of_an_upstream_subdirectory() {
    let fixture = Fixture::new();
    let upstream = fixture.0.join("upstream files");
    let exports = fixture.0.join("exports");
    let nested = exports.join("nested");
    fs::create_dir_all(upstream.join("private")).unwrap();
    fs::create_dir_all(&nested).unwrap();
    let input = local_bot_api_config(&upstream, &exports, &fixture.0.join("relay.sqlite"));
    let config = Config::parse(&input, None).unwrap();

    // Model a bind of an upstream descendant below exports without mounting anything.
    // The configured roots and their ancestors share only the ordinary fixture prefix.
    let mountinfo = |source: &Path| {
        format!(
            "1 0 8:1 / / rw - ext4 /dev/synthetic rw\n\
             2 1 8:2 /data {} rw - ext4 /dev/synthetic2 rw\n\
             3 2 8:2 {} {} rw - ext4 /dev/synthetic2 rw\n\
             4 2 0:4 mnt:[4026537000] {} rw - nsfs nsfs rw\n",
            mount_field(&fixture.0),
            mount_field(source),
            mount_field(&nested),
            mount_field(&exports.join("namespace"))
        )
    };
    assert!(
        config
            .validate_local_path_aliases(&upstream, mountinfo(Path::new("/data/other")).as_bytes())
            .is_ok()
    );
    assert_eq!(
        config.validate_local_path_aliases(
            &upstream,
            mountinfo(Path::new("/data/upstream files/private")).as_bytes()
        ),
        Err(AppError::InvalidConfig(LOCAL_BOT_API_FILES_ROOT))
    );
}

#[test]
fn protected_files_and_generated_bearer_are_not_overwritten() {
    let fixture = Fixture::new();
    fixture.write("bot", format!("{BOT}\n").as_bytes());
    let path = fixture.0.join("bearer");
    let config = fixture.config(&format!(
        "[server]\nauth_token_file={path:?}\ncreate_auth_token_file=true"
    ));
    let first = config.resolve_secrets(|_| None).unwrap();
    let second = config.resolve_secrets(|_| None).unwrap();
    assert_eq!(first.bot_token.expose(), BOT);
    let token = first.auth_token.unwrap();
    assert_eq!(token.expose().len(), 64);
    assert_eq!(token.expose(), second.auth_token.unwrap().expose());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
    assert!(config.resolve_secrets(|_| None).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), token.expose());
    fs::set_permissions(&fixture.0, Permissions::from_mode(0o755)).unwrap();
    assert!(config.resolve_secrets(|_| None).is_err());
}

#[test]
fn unsafe_secret_files_fail_closed() {
    let fixture = Fixture::new();
    let bot = fixture.write("bot", BOT.as_bytes());
    let config = fixture.config("");
    for mode in [0o000, 0o640, 0o604, 0o666] {
        fs::set_permissions(&bot, Permissions::from_mode(mode)).unwrap();
        assert!(config.resolve_secrets(|_| None).is_err());
    }
    fs::set_permissions(&bot, Permissions::from_mode(0o600)).unwrap();
    fs::rename(&bot, fixture.0.join("target")).unwrap();
    symlink(fixture.0.join("target"), &bot).unwrap();
    assert!(config.resolve_secrets(|_| None).is_err());
    fs::remove_file(&bot).unwrap();
    fs::hard_link(fixture.0.join("target"), &bot).unwrap();
    assert!(config.resolve_secrets(|_| None).is_err());
    fs::remove_file(&bot).unwrap();
    fs::create_dir(&bot).unwrap();
    assert!(config.resolve_secrets(|_| None).is_err());
    fs::remove_dir(&bot).unwrap();
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &bot,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    assert!(config.resolve_secrets(|_| None).is_err());
    fs::remove_file(&bot).unwrap();
    fixture.write("bot", &vec![b'x'; 4_099]);
    assert!(config.resolve_secrets(|_| None).is_err());
}

#[test]
fn errors_and_debug_redact_input_and_paths() {
    let marker = "synthetic-private-value";
    let error = parse(&format!("[server]\nauth_token='{marker}'")).unwrap_err();
    assert_eq!(error.code(), "CONFIG_INVALID");
    assert!(!format!("{error} {error:?}").contains(marker));
    let config = parse("[server]\nauth_token_env='AUTH'").unwrap();
    for value in ["", "bad\nheader", "short"] {
        assert!(
            config
                .resolve_secrets(|name| Some(if name == "AUTH" { value } else { BOT }.into()))
                .is_err()
        );
    }
    let auth = "a".repeat(32);
    let secrets = config
        .resolve_secrets(|name| Some(if name == "AUTH" { auth.as_str() } else { BOT }.into()))
        .unwrap();
    let debug = format!("{config:?} {secrets:?}");
    assert!(!debug.contains(BOT) && !debug.contains(&auth));
    assert!(parse("").unwrap().resolve_secrets(|_| None).is_err());
    let fixture = Fixture::new();
    let config = fixture.config("");
    let error = config.resolve_secrets(|_| None).unwrap_err();
    assert!(!format!("{error} {error:?}").contains(fixture.0.to_str().unwrap()));
    let path = fixture.write("oversized.toml", &vec![b' '; CONFIG_BYTES + 1]);
    assert!(Config::load(&path).is_err());
}
