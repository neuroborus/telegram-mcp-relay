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

#[test]
fn defaults_and_example_are_safe_and_independent() {
    let config = parse("").unwrap();
    assert!(config.server.listen.ip().is_loopback());
    assert!(config.delivery.wait_fallback_enabled && config.activity.typing_enabled);
    assert_eq!(config.delivery.wait_timeout_seconds, 600);
    assert!(!config.history.enabled);
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
