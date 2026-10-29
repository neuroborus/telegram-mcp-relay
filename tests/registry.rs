use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf};

use rusqlite::Connection;
use telegram_mcp_relay::{
    config::{Limits, Storage},
    domain::outbound::format_text,
    error::AppError,
    repositories::{Database, RouteKind},
};

const DAY: i64 = 86_400_000;

struct Fixture(Storage);

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("LOCAL_ARTIFACTS")
            .join(format!("registry-test-{:016x}", u64::from_ne_bytes(random)));
        fs::create_dir_all(directory.parent().unwrap()).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        Self(Storage {
            path: directory.join("relay.sqlite"),
            ..Storage::default()
        })
    }

    fn open(&self, limits: &Limits) -> Database {
        Database::open(&self.0, limits).unwrap()
    }

    fn sql(&self) -> Connection {
        Connection::open(&self.0.path).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.path.parent().unwrap()).unwrap();
    }
}

#[test]
fn same_titles_interleaved_text_and_documents_survive_rename_and_restart() {
    let fixture = Fixture::new();
    let limits = Limits::default();
    let mut database = fixture.open(&limits);
    let mut keys = Vec::new();
    for external in ["thread-a", "thread-b", "thread-c"] {
        let session = database
            .register_session(Some(external), None, " Same\n title ", 100)
            .unwrap();
        assert_eq!(session.title.as_str(), "Same title");
        assert_eq!(session.session_key.len(), 64);
        assert!(session.session_key.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!keys.contains(&session.session_key));
        keys.push(session.session_key);
    }
    for (key, chat, message, kind) in [
        (&keys[0], -101, 7, RouteKind::Text),
        (&keys[1], -101, 8, RouteKind::Document),
        (&keys[2], -102, 7, RouteKind::Document),
        (&keys[0], -101, 9, RouteKind::Document),
    ] {
        let reservation = database.reserve_route(key).unwrap();
        database
            .persist_route(reservation, chat, message, kind, 150, 200)
            .unwrap();
    }
    let old_text = format_text(&database.session(&keys[0]).unwrap().title, "Synthetic").unwrap();
    let renamed = database
        .register_session(Some("thread-a"), None, " New\t title ", 300)
        .unwrap();
    assert_eq!(renamed.session_key, keys[0]);
    assert_eq!(old_text, "[Same title]\nSynthetic");
    assert_eq!(
        format_text(&renamed.title, "Synthetic").unwrap(),
        "[New title]\nSynthetic"
    );
    assert_eq!(
        renamed.inbox_uri(),
        format!("telegram-bridge://sessions/{}/inbox", keys[0])
    );
    drop(database);
    let mut database = fixture.open(&limits);
    assert_eq!(database.session(&keys[0]).unwrap(), renamed);
    for (chat, message, index, kind) in [
        (-101, 7, 0, RouteKind::Text),
        (-101, 8, 1, RouteKind::Document),
        (-102, 7, 2, RouteKind::Document),
        (-101, 9, 0, RouteKind::Document),
    ] {
        let route = database.route(chat, message, 400).unwrap().unwrap();
        assert_eq!(route.session_key, keys[index]);
        assert_eq!(route.kind, kind);
        assert_eq!(route.created_at, 150);
    }
    assert_eq!(database.route(-102, 8, 400).unwrap(), None);
    assert_eq!(
        database
            .register_session(Some("thread-a"), Some(&keys[0]), "New title", 400)
            .unwrap()
            .created_at,
        100
    );
}

#[test]
fn saved_handle_resumes_anonymous_identity_and_conflicts_never_create_sessions() {
    let fixture = Fixture::new();
    let limits = Limits::default();
    let mut database = fixture.open(&limits);
    let anonymous = database.register_session(None, None, "", 100).unwrap();
    let another = database
        .register_session(Some("stable"), None, "", 100)
        .unwrap();
    assert_ne!(anonymous.session_key, another.session_key);
    drop(database);
    let mut database = fixture.open(&limits);
    let resumed = database
        .register_session(None, Some(&anonymous.session_key), "Renamed", 200)
        .unwrap();
    assert_eq!(resumed.session_key, anonymous.session_key);
    assert_eq!(resumed.external_session_id, None);
    assert_eq!(resumed.created_at, 100);
    assert_eq!(
        database.register_session(Some("stable"), Some(&anonymous.session_key), "wrong", 300),
        Err(AppError::SessionIdentityConflict)
    );
    assert_eq!(
        database.register_session(Some("stable"), Some("unknown"), "wrong", 300),
        Err(AppError::SessionNotFound)
    );
    assert_eq!(database.session(&anonymous.session_key).unwrap(), resumed);
    let count: i64 = fixture
        .sql()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn session_activity_and_route_expiry_have_independent_anchors() {
    let mut fixture = Fixture::new();
    fixture.0.session_ttl_days = 1;
    fixture.0.route_ttl_days = 2;
    let mut database = fixture.open(&Limits::default());
    let key = database
        .register_session(None, None, "Synthetic", 100)
        .unwrap()
        .session_key;
    let reservation = database.reserve_route(&key).unwrap();
    assert_eq!(database.session(&key).unwrap().last_activity_at, 100);
    database
        .persist_route(reservation, -101, 7, RouteKind::Text, 200, 500)
        .unwrap();
    let sent = database.session(&key).unwrap();
    assert_eq!((sent.last_activity_at, sent.expires_at), (500, 500 + DAY));
    let route = database.route(-101, 7, 500 + DAY).unwrap().unwrap();
    assert_eq!((route.created_at, route.expires_at), (200, 200 + 2 * DAY));
    // A read/expired session does not expire its still-valid child route or refresh activity.
    assert_eq!(database.session(&key).unwrap(), sent);
    let resumed = database
        .register_session(None, Some(&key), "Renamed", 3 * DAY)
        .unwrap();
    assert_eq!(resumed.expires_at, 4 * DAY);
    assert!(
        database
            .route(-101, 7, route.expires_at - 1)
            .unwrap()
            .is_some()
    );
    assert_eq!(database.route(-101, 7, route.expires_at).unwrap(), None);
    assert_eq!(
        database
            .register_session(None, Some(&key), "Renamed", 200)
            .unwrap(),
        resumed
    );
}

#[test]
fn count_limits_include_retained_rows_and_simultaneous_sends() {
    let fixture = Fixture::new();
    let limits = Limits {
        sessions: 1,
        routes: 1,
        ..Limits::default()
    };
    let mut database = fixture.open(&limits);
    let session = database
        .register_session(Some("stable"), None, "Synthetic", 100)
        .unwrap();
    let key = &session.session_key;
    assert_eq!(
        database.register_session(None, None, "Other", 40 * DAY),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(database.session(key).unwrap(), session);
    assert_eq!(
        database
            .register_session(Some("stable"), None, "Synthetic", 100)
            .unwrap(),
        session
    );
    let reservation = database.reserve_route(key).unwrap();
    assert!(matches!(
        database.reserve_route(key),
        Err(AppError::StorageCapacity)
    ));
    drop(reservation); // Caller cancellation/definitive failure releases admission.
    let reservation = database.reserve_route(key).unwrap();
    database
        .persist_route(reservation, -101, 7, RouteKind::Text, 100, 100)
        .unwrap();
    assert!(matches!(
        database.reserve_route(key),
        Err(AppError::StorageCapacity)
    ));
    assert_eq!(database.route(-101, 7, 40 * DAY).unwrap(), None);
    assert!(matches!(
        database.reserve_route(key),
        Err(AppError::StorageCapacity)
    ));
}

#[test]
fn metadata_budget_counts_utf8_and_reservations_and_rejects_rename_atomically() {
    let fixture = Fixture::new();
    // Exactly one session (including its external ID/title) and one route.
    let session_bytes = 128 + 64 + "外部".len() + "界".len();
    let limits = Limits {
        metadata_bytes: (session_bytes + 128 + 64) as u32,
        ..Limits::default()
    };
    let mut database = fixture.open(&limits);
    let session = database
        .register_session(Some("外部"), None, "界", 100)
        .unwrap();
    let key = &session.session_key;
    // A schema-valid legacy title must be charged by stored bytes before normalization.
    fixture
        .sql()
        .execute("UPDATE sessions SET title = char(10)", [])
        .unwrap();
    assert_eq!(
        database
            .register_session(Some("外部"), None, "界", 100)
            .unwrap(),
        session
    );
    let reservation = database.reserve_route(key).unwrap();
    assert_eq!(
        database.register_session(Some("外部"), None, "界界", 200),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(database.session(key).unwrap(), session);
    assert!(matches!(
        database.reserve_route(key),
        Err(AppError::StorageCapacity)
    ));
    drop(reservation);
    database
        .register_session(Some("外部"), None, "界界", 200)
        .unwrap();
    assert!(matches!(
        database.reserve_route(key),
        Err(AppError::StorageCapacity)
    ));
    database
        .register_session(Some("外部"), None, "界", 300)
        .unwrap();
    let reservation = database.reserve_route(key).unwrap();
    database
        .persist_route(reservation, -101, 7, RouteKind::Document, 300, 300)
        .unwrap();
    assert!(matches!(
        database.reserve_route(key),
        Err(AppError::StorageCapacity)
    ));
    drop(database);
    let mut database = fixture.open(&Limits {
        metadata_bytes: (session_bytes - 1) as u32,
        ..limits
    });
    assert!(matches!(
        database.register_session(Some("外部"), None, "界", 400),
        Err(AppError::StorageCapacity)
    ));
    assert!(database.route(-101, 7, 400).unwrap().is_some());
}

#[test]
fn route_conflicts_and_failed_bookkeeping_do_not_mutate_routes_or_activity() {
    let fixture = Fixture::new();
    let mut database = fixture.open(&Limits::default());
    let a = database.register_session(None, None, "A", 100).unwrap();
    let b = database.register_session(None, None, "B", 100).unwrap();
    let reservation = database.reserve_route(&a.session_key).unwrap();
    database
        .persist_route(reservation, -101, 7, RouteKind::Text, 100, 200)
        .unwrap();
    let original = database.route(-101, 7, 300).unwrap();
    for key in [&a.session_key, &b.session_key] {
        let reservation = database.reserve_route(key).unwrap();
        assert_eq!(
            database.persist_route(reservation, -101, 7, RouteKind::Document, 300, 300),
            Err(AppError::RouteConflict)
        );
    }
    assert_eq!(database.route(-101, 7, 300).unwrap(), original);
    assert_eq!(database.session(&b.session_key).unwrap(), b);
    fixture
        .sql()
        .execute_batch(
            "CREATE TRIGGER fail_activity BEFORE UPDATE ON sessions
        BEGIN SELECT RAISE(ABORT, 'synthetic private error'); END",
        )
        .unwrap();
    let reservation = database.reserve_route(&a.session_key).unwrap();
    assert_eq!(
        database.persist_route(reservation, -101, 8, RouteKind::Text, 300, 500),
        Err(AppError::StorageFailure)
    );
    assert_eq!(database.route(-101, 8, 500).unwrap(), None);
    assert_eq!(
        database.session(&a.session_key).unwrap().last_activity_at,
        200
    );
}

#[test]
fn invalid_inputs_and_foreign_reservations_fail_without_effects_or_private_diagnostics() {
    let fixture = Fixture::new();
    let mut database = fixture.open(&Limits::default());
    let session = database
        .register_session(None, None, "Private title", 100)
        .unwrap();
    for timestamp in [-1, i64::MAX] {
        assert!(matches!(
            database.register_session(None, Some(&session.session_key), "wrong", timestamp),
            Err(AppError::InvalidRequest(_))
        ));
    }
    for external in ["", "synthetic\0id"] {
        assert_eq!(
            database.register_session(Some(external), None, "wrong", 100),
            Err(AppError::InvalidRequest("external_session_id"))
        );
    }
    assert!(matches!(
        database.reserve_route("unknown"),
        Err(AppError::SessionNotFound)
    ));
    for (chat, message) in [(0, 7), (-101, 0), (-101, -1)] {
        let reservation = database.reserve_route(&session.session_key).unwrap();
        assert!(matches!(
            database.persist_route(reservation, chat, message, RouteKind::Text, 100, 100),
            Err(AppError::InvalidRequest(_))
        ));
    }
    let other_fixture = Fixture::new();
    let mut other = other_fixture.open(&Limits::default());
    let reservation = database.reserve_route(&session.session_key).unwrap();
    let debug = format!("{session:?} {reservation:?}");
    assert!(!debug.contains(&session.session_key));
    assert!(!debug.contains("Private title"));
    assert!(matches!(
        other.persist_route(reservation, -101, 7, RouteKind::Text, 100, 100),
        Err(AppError::InvalidRequest(_))
    ));
    assert_eq!(database.session(&session.session_key).unwrap(), session);
    assert_eq!(database.route(-101, 7, 100).unwrap(), None);
}
