use super::*;
use crate::{repositories::RouteKind, services::ingress::IngressService};
use serde_json::json;
use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf};

struct Fixture {
    root: PathBuf,
    config: Config,
    db: Arc<Mutex<Database>>,
    owner: String,
    observer: String,
    reply: String,
    handle: String,
}

impl Fixture {
    fn new(shared: bool) -> Self {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("attachments-{:032x}", u128::from_ne_bytes(random)));
        for path in [root.clone(), root.join("exports"), root.join("downloads")] {
            fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        let mut config = Config::default();
        config.storage.path = root.join("relay.sqlite");
        config.files.allowed_roots = vec![root.join("exports")];
        config.downloads.enabled = true;
        config.downloads.roots = vec![root.join("downloads")];
        config.downloads.timeout_seconds = 1;
        config.telegram.chat_id = -101;
        config.telegram.allowed_chat_ids = vec![-101];
        config.telegram.allowed_user_ids = vec![7];
        config.history.enabled = shared;
        config.server.auth_token_env = shared.then(|| "SYNTHETIC_AUTH".to_owned());
        let mut db =
            Database::open_with_history(&config.storage, &config.limits, &config.history, shared)
                .unwrap();
        let now = unix_millis().unwrap();
        let owner = db
            .register_session(None, None, "Owner", None, now)
            .unwrap()
            .session_key;
        let observer = db
            .register_session(None, None, "Observer", None, now)
            .unwrap()
            .session_key;
        let reservation = db.reserve_route(&owner).unwrap();
        db.persist_route(reservation, -101, 11, RouteKind::Text, now, now)
            .unwrap();
        let update = serde_json::from_value(json!({"update_id":1,"message":{
            "message_id":23,"date":now/1000,"chat":{"id":-101},"from":{"id":7,"is_bot":false},
            "reply_to_message":{"message_id":11},"document":{"file_id":"private-file",
            "file_unique_id":"stable","file_name":"../../untrusted.txt","mime_type":"text/plain","file_size":1}}})).unwrap();
        IngressService::new(&config)
            .unwrap()
            .ingest(&mut db, &update, now)
            .unwrap();
        let reply = db.inbox_snapshot(&owner, None, 1, now).unwrap().replies[0]
            .reply_id
            .clone();
        let handle = db
            .own_attachment_handles(&owner, &reply, now)
            .unwrap()
            .remove(0);
        Self {
            root,
            config,
            db: Arc::new(Mutex::new(db)),
            owner,
            observer,
            reply,
            handle,
        }
    }

    fn service(&self) -> AttachmentService {
        let client =
            TelegramClient::for_test(&self.config, "http://127.0.0.1:1/botSynthetic/".into())
                .unwrap();
        AttachmentService::new(self.db.clone(), client, &self.config).unwrap()
    }
    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.config.storage.path).unwrap()
    }
    fn request(&self, session: &str, handle: &str) -> GetAttachmentRequest {
        GetAttachmentRequest {
            session_key: session.into(),
            attachment_handle: handle.into(),
        }
    }
    fn untouched(&self) {
        assert_eq!(
            fs::read_dir(self.root.join("downloads")).unwrap().count(),
            0
        );
        let db = self.db.lock().unwrap();
        assert_eq!(db.update_cursor().unwrap(), Some(1));
        assert!(db.download_records("").unwrap().is_empty());
        assert_eq!(
            db.inbox_snapshot(&self.owner, None, 1, unix_millis().unwrap())
                .unwrap()
                .replies
                .len(),
            1
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[tokio::test]
async fn own_provenance_is_observational_and_independent_of_history_or_claims() {
    let fixture = Fixture::new(false);
    let service = fixture.service();
    {
        let mut db = fixture.db.lock().unwrap();
        let now = unix_millis().unwrap();
        let before = db.inbox_snapshot(&fixture.owner, None, 1, now).unwrap();
        let metadata = service
            .own_files(&db, &fixture.owner, &fixture.reply, now)
            .unwrap();
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].attachment_handle, fixture.handle);
        assert_eq!(metadata[0].filename.as_deref(), Some("../../untrusted.txt"));
        assert!(
            !serde_json::to_string(&metadata)
                .unwrap()
                .contains("private-file")
        );
        assert_eq!(
            db.inbox_snapshot(&fixture.owner, None, 1, now).unwrap(),
            before
        );
        assert!(
            service
                .own_files(&db, &fixture.observer, &fixture.reply, now)
                .unwrap()
                .is_empty()
        );
        db.claim_replies(&fixture.owner, None, 1, 1000, now)
            .unwrap();
        assert!(
            service
                .authorize(&db, &fixture.owner, &fixture.handle, now)
                .is_ok()
        );
    }
    service.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn forged_foreign_expired_legacy_and_changed_source_handles_fail_before_effects() {
    let fixture = Fixture::new(false);
    let mut service = fixture.service();
    for (session, handle) in [
        (&fixture.observer, fixture.handle.as_str()),
        (&fixture.owner, "https://host/file"),
        (&fixture.owner, "private-file"),
        (&fixture.owner, &"f".repeat(64)),
    ] {
        assert!(matches!(
            service.download(fixture.request(session, handle)).await,
            Err(AppError::AttachmentUnavailable)
        ));
        fixture.untouched();
    }
    let expiry: i64 = fixture
        .sql()
        .query_row("SELECT expires_at FROM attachment_records", [], |r| {
            r.get(0)
        })
        .unwrap();
    for change in [
        "UPDATE attachment_records SET expires_at = 0",
        "UPDATE attachment_records SET sender_user_id = NULL",
        "UPDATE replies SET telegram_message_id = 24",
    ] {
        fixture.sql().execute_batch(change).unwrap();
        assert!(matches!(
            service
                .download(fixture.request(&fixture.owner, &fixture.handle))
                .await,
            Err(AppError::AttachmentUnavailable)
        ));
        fixture.untouched();
        fixture
            .sql()
            .execute(
                "UPDATE attachment_records SET expires_at = ?1, sender_user_id = 7",
                [expiry],
            )
            .unwrap();
        fixture
            .sql()
            .execute_batch("UPDATE replies SET telegram_message_id = 23")
            .unwrap();
    }
    service.users.clear();
    assert!(matches!(
        service
            .download(fixture.request(&fixture.owner, &fixture.handle))
            .await,
        Err(AppError::AttachmentUnavailable)
    ));
    service.users.push(7);
    service.chats.clear();
    assert!(matches!(
        service
            .download(fixture.request(&fixture.owner, &fixture.handle))
            .await,
        Err(AppError::AttachmentUnavailable)
    ));
    fixture.untouched();
    service.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn queued_work_revalidates_provenance_before_any_file_or_network_effects() {
    let fixture = Fixture::new(false);
    let service = fixture.service();
    let mut call = Box::pin(service.download(fixture.request(&fixture.owner, &fixture.handle)));
    tokio::select! { biased;
        _ = &mut call => panic!("download completed before its worker ran"),
        _ = std::future::ready(()) => (),
    }
    fixture
        .sql()
        .execute_batch("UPDATE attachment_records SET expires_at = 0")
        .unwrap();
    assert!(matches!(call.await, Err(AppError::AttachmentUnavailable)));
    fixture.untouched();
    service.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_access_requires_current_history_user_chat_and_matching_journal_provenance() {
    let fixture = Fixture::new(true);
    let mut service = fixture.service();
    let now = unix_millis().unwrap();
    assert!(
        service
            .authorize(
                &fixture.db.lock().unwrap(),
                &fixture.observer,
                &fixture.handle,
                now
            )
            .is_ok()
    );
    service.users.clear();
    assert!(matches!(
        service
            .download(fixture.request(&fixture.observer, &fixture.handle))
            .await,
        Err(AppError::AttachmentUnavailable)
    ));
    service.users.push(7);
    service.chats.clear();
    assert!(matches!(
        service
            .download(fixture.request(&fixture.observer, &fixture.handle))
            .await,
        Err(AppError::AttachmentUnavailable)
    ));
    service.chats.push(-101);
    service.shared = false;
    assert!(matches!(
        service
            .download(fixture.request(&fixture.observer, &fixture.handle))
            .await,
        Err(AppError::AttachmentUnavailable)
    ));
    // Disabling shared access must leave the live own-reply grant intact.
    assert!(
        service
            .authorize(
                &fixture.db.lock().unwrap(),
                &fixture.owner,
                &fixture.handle,
                now
            )
            .is_ok()
    );
    service.shared = true;
    let disabled = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    assert!(matches!(
        service.authorize(&disabled, &fixture.observer, &fixture.handle, now),
        Err(AppError::AttachmentUnavailable)
    ));
    fixture
        .sql()
        .execute_batch("UPDATE journal_records SET sender_user_id = 8")
        .unwrap();
    assert!(matches!(
        service
            .download(fixture.request(&fixture.observer, &fixture.handle))
            .await,
        Err(AppError::AttachmentUnavailable)
    ));
    fixture.untouched();
    service.store.shutdown().await.unwrap();
}
