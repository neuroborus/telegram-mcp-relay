use super::*;
use rustix::fs::{RenameFlags, renameat_with};
use std::{
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink},
    path::Path,
    sync::atomic::{AtomicBool, AtomicI64, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Fixture {
    base: PathBuf,
    config: Config,
    clock: Arc<AtomicI64>,
}

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "download-test-{:032x}",
            u128::from_ne_bytes(random)
        ));
        DirBuilder::new().mode(0o700).create(&base).unwrap();
        for name in ["downloads", "exports"] {
            DirBuilder::new()
                .mode(0o700)
                .create(base.join(name))
                .unwrap();
        }
        let mut config = Config::default();
        config.storage.path = base.join("relay.sqlite");
        config.files.allowed_roots = vec![base.join("exports")];
        let unit = rustix::fs::statvfs(base.join("downloads"))
            .unwrap()
            .f_frsize
            .max(512);
        config.downloads = Downloads {
            enabled: true,
            roots: vec![base.join("downloads")],
            max_file_bytes: 1_024,
            max_total_bytes: 1_024_u64.div_ceil(unit) * unit * 16,
            max_files: 8,
            concurrent: 4,
            retention_seconds: 10,
            min_free_bytes: 1,
            cleanup_interval_seconds: 1,
            ..Downloads::default()
        };
        Self {
            base,
            config,
            clock: Arc::new(AtomicI64::new(1_000)),
        }
    }

    fn database(&self) -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(
            Database::open(&self.config.storage, &self.config.limits).unwrap(),
        ))
    }
    fn store(&self, db: Arc<Mutex<Database>>) -> DownloadStore {
        let clock = self.clock.clone();
        DownloadStore::build(
            db,
            &self.config,
            Arc::new(move || Ok(clock.load(Ordering::SeqCst))),
        )
        .unwrap()
    }
    fn root(&self) -> &Path {
        &self.config.downloads.roots[0]
    }
    fn advance(&self) {
        self.clock.fetch_add(11_000, Ordering::SeqCst);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).unwrap();
    }
}

async fn save(store: &DownloadStore, bytes: &'static [u8]) -> Result<DownloadedFile, AppError> {
    store
        .materialize(move |file, _, _| {
            Box::pin(async move {
                file.write_all(bytes)
                    .await
                    .map_err(|_| AppError::DownloadWriteFailed)?;
                file.flush()
                    .await
                    .map_err(|_| AppError::DownloadWriteFailed)?;
                Ok(bytes.len() as u64)
            })
        })
        .await
}

fn records(db: &Arc<Mutex<Database>>) -> Vec<DownloadRecord> {
    db.lock().unwrap().download_records("").unwrap()
}

#[tokio::test]
async fn private_download_composes_the_real_adapter_with_the_reserved_inode() {
    let fixture = Fixture::new();
    let store = fixture.store(fixture.database());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TelegramClient::for_test(
        &fixture.config,
        format!("http://{}/botSynthetic/", listener.local_addr().unwrap()),
    )
    .unwrap();
    let peer = tokio::spawn(async move {
        for (request_line, body) in [
            ("POST /botSynthetic/getFile HTTP/1.1", br#"{"ok":true,"result":{"file_id":"stored-id","file_unique_id":"stable-id","file_size":8,"file_path":"documents/file.bin"}}"#.as_slice()),
            ("GET /file/botSynthetic/documents/file.bin HTTP/1.1", b"download".as_slice()),
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut input = Vec::new();
            loop {
                let mut chunk = [0; 1_024];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0 && input.len() + read <= 8_192);
                input.extend_from_slice(&chunk[..read]);
                let Some(end) = input.windows(4).position(|part| part == b"\r\n\r\n") else { continue; };
                let headers = std::str::from_utf8(&input[..end]).unwrap();
                assert_eq!(headers.lines().next().unwrap(), request_line);
                let length = headers.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                }).unwrap_or(0);
                if input.len() != end + 4 + length { continue; }
                if length > 0 {
                    assert_eq!(serde_json::from_slice::<serde_json::Value>(&input[end + 4..]).unwrap(), serde_json::json!({"file_id":"stored-id"}));
                }
                break;
            }
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
        }
    });
    let output = store
        .download(client, "stored-id".into(), "stable-id".into(), || Ok(()))
        .await
        .unwrap();
    assert_eq!(output.bytes, 8);
    assert_eq!(fs::read(output.path).unwrap(), b"download");
    peer.await.unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn completed_files_are_exclusive_private_bounded_and_retired_without_touching_unknown_files()
{
    let fixture = Fixture::new();
    fs::write(fixture.root().join("source.txt"), b"user-owned source").unwrap();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let output = save(&store, b"synthetic bytes").await.unwrap();
    assert_eq!(fs::read(&output.path).unwrap(), b"synthetic bytes");
    assert!(output.path.starts_with(fixture.root()));
    assert_eq!(
        output.path.file_name().unwrap(),
        filename(&output.id).unwrap().as_str()
    );
    let metadata = fs::metadata(&output.path).unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(identity(&metadata), output.identity);
    assert_eq!((output.bytes, output.expires_at), (15, 11_000));
    let row = &records(&db)[0];
    assert!(row.complete);
    assert_eq!((row.reserved_bytes, row.actual_bytes), (15, Some(15)));
    let diagnostic = format!("{store:?} {output:?} {row:?}");
    assert!(!diagnostic.contains(fixture.base.to_str().unwrap()));
    assert!(!diagnostic.contains(&output.id));
    fixture.advance();
    store.cleanup(false).unwrap();
    assert!(!output.path.exists());
    assert!(records(&db).is_empty());
    assert_eq!(
        fs::read(fixture.root().join("source.txt")).unwrap(),
        b"user-owned source"
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn count_byte_and_disk_reservations_fail_before_effects_and_recover_after_expiry() {
    for byte_budget in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.downloads.max_files = if byte_budget { 8 } else { 1 };
        fixture.config.downloads.concurrent = 1;
        let unit = rustix::fs::statvfs(fixture.root())
            .unwrap()
            .f_frsize
            .max(512);
        let charge = fixture.config.downloads.max_file_bytes.div_ceil(unit) * unit;
        if byte_budget {
            fixture.config.downloads.max_total_bytes = charge;
        }
        let db = fixture.database();
        let store = fixture.store(db.clone());
        save(&store, b"x").await.unwrap();
        assert_eq!(
            save(&store, b"y").await.unwrap_err(),
            AppError::DownloadCapacity
        );
        assert_eq!(records(&db).len(), 1);
        assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 1);
        fixture.advance();
        store.cleanup(false).unwrap();
        save(&store, b"y").await.unwrap();
        store.shutdown().await.unwrap();
    }
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    assert_eq!(
        db.lock()
            .unwrap()
            .reserve_download(
                &store.0.roots[0].identity,
                &fixture.config.downloads,
                1_000,
                1_024,
                4_096,
                u64::MAX,
            )
            .unwrap_err(),
        AppError::DownloadCapacity
    );
    assert!(records(&db).is_empty());
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
    let reserve = |available_files| {
        db.lock().unwrap().reserve_download(
            &store.0.roots[0].identity,
            &fixture.config.downloads,
            1_000,
            u64::MAX,
            4_096,
            available_files,
        )
    };
    assert_eq!(reserve(0).unwrap_err(), AppError::DownloadCapacity);
    reserve(1).unwrap();
    assert_eq!(reserve(1).unwrap_err(), AppError::DownloadCapacity);
    assert_eq!(records(&db).len(), 1);
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
    store.shutdown().await.unwrap();
    assert!(records(&db).is_empty());
}

#[tokio::test]
async fn partial_byte_reservations_hold_until_caller_cancellation_drains_without_shutdown() {
    let mut fixture = Fixture::new();
    fixture.config.downloads.concurrent = 2;
    let unit = rustix::fs::statvfs(fixture.root())
        .unwrap()
        .f_frsize
        .max(512);
    fixture.config.downloads.max_total_bytes =
        fixture.config.downloads.max_file_bytes.div_ceil(unit) * unit;
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let (entered, observed) = oneshot::channel();
    let caller = store.clone();
    let call = tokio::spawn(async move {
        caller
            .materialize(move |file, _, _| {
                Box::pin(async move {
                    file.write_all(b"partial").await.unwrap();
                    file.flush().await.unwrap();
                    entered.send(()).unwrap();
                    std::future::pending::<Result<u64, AppError>>().await
                })
            })
            .await
    });
    observed.await.unwrap();
    assert_eq!(
        save(&store, b"other").await.unwrap_err(),
        AppError::DownloadCapacity
    );
    assert_eq!(records(&db).len(), 1);
    // Closing this observer lets wait observe drainage; it neither cancels jobs nor closes
    // admission. Caller cancellation must independently stop and retire the active writer.
    store.0.tasks.close();
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    store.0.tasks.wait().await;
    assert!(records(&db).is_empty());
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
    store.0.tasks.reopen();
    save(&store, b"capacity recovered").await.unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn canceling_an_unreceived_completed_result_retires_its_owned_file() {
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let mut call = Box::pin(save(&store, b"completed but undisclosed"));
    // Register the worker, then leave the caller unpolled until its result is queued.
    tokio::select! { biased;
        result = &mut call => panic!("request completed before scheduling: {result:?}"),
        _ = std::future::ready(()) => (),
    }
    store.0.tasks.close();
    store.0.tasks.wait().await;
    assert!(records(&db)[0].complete);
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 1);
    drop(call);
    assert!(records(&db).is_empty());
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
    store.0.tasks.reopen();
    save(&store, b"capacity recovered").await.unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn partial_write_and_size_failure_remove_owned_output_and_keep_other_durable_data() {
    let fixture = Fixture::new();
    let db = fixture.database();
    let session = db
        .lock()
        .unwrap()
        .register_session(Some("synthetic-session"), None, "Preserved", None, 1_000)
        .unwrap();
    let store = fixture.store(db.clone());
    for oversized in [false, true] {
        let error = store
            .materialize(move |file, limit, _| {
                Box::pin(async move {
                    file.write_all(b"partial").await.unwrap();
                    file.flush().await.unwrap();
                    if oversized {
                        Ok(limit + 1)
                    } else {
                        Err(AppError::DownloadWriteFailed)
                    }
                })
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AppError::DownloadWriteFailed | AppError::FileTooLarge { .. }
        ));
        assert!(records(&db).is_empty());
        assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
        assert_eq!(
            db.lock()
                .unwrap()
                .session(&session.session_key)
                .unwrap()
                .title
                .as_str(),
            "Preserved"
        );
    }
    save(&store, b"success").await.unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn canceled_and_shutdown_work_keeps_capacity_until_writes_retire_then_releases_it() {
    for cancel in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.downloads.concurrent = 1;
        let db = fixture.database();
        let store = fixture.store(db.clone());
        let (entered, observed) = oneshot::channel();
        let caller = store.clone();
        let call = tokio::spawn(async move {
            caller
                .materialize(move |file, _, _| {
                    Box::pin(async move {
                        file.write_all(b"partial").await.unwrap();
                        file.flush().await.unwrap();
                        entered.send(()).unwrap();
                        std::future::pending::<Result<u64, AppError>>().await
                    })
                })
                .await
        });
        observed.await.unwrap();
        assert_eq!(records(&db)[0].reserved_bytes, 1_024);
        assert_eq!(
            save(&store, b"other").await.unwrap_err(),
            AppError::DownloadCapacity
        );
        if cancel {
            call.abort();
        }
        store.shutdown().await.unwrap();
        if cancel {
            assert!(call.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(call.await.unwrap().unwrap_err(), AppError::ServiceStopped);
        }
        assert!(records(&db).is_empty());
        assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
        assert_eq!(
            save(&store, b"closed").await.unwrap_err(),
            AppError::ServiceStopped
        );
        drop(store);
        let reopened = fixture.store(db.clone());
        save(&reopened, b"capacity recovered").await.unwrap();
        reopened.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn admission_deadline_includes_time_before_the_worker_is_first_scheduled() {
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let written = Arc::new(AtomicBool::new(false));
    let writing = written.clone();
    tokio::time::pause();
    let mut call = Box::pin(store.materialize(move |_, _, _| {
        Box::pin(async move {
            writing.store(true, Ordering::SeqCst);
            Ok(0)
        })
    }));
    // Register work without yielding to its worker, then expire its admission deadline.
    tokio::select! { biased;
        result = &mut call => panic!("request completed before scheduling: {result:?}"),
        _ = std::future::ready(()) => (),
    }
    tokio::time::advance(Duration::from_secs(601)).await;
    assert_eq!(
        call.await.unwrap_err(),
        crate::error::TelegramError::Deadline.into()
    );
    assert!(!written.load(Ordering::SeqCst));
    assert!(records(&db).is_empty());
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn total_deadline_retires_a_stalled_writer_without_leaking_its_reservation() {
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let (entered, observed) = oneshot::channel();
    let caller = store.clone();
    let call = tokio::spawn(async move {
        caller
            .materialize(move |file, _, _| {
                Box::pin(async move {
                    file.write_all(b"partial").await.unwrap();
                    file.flush().await.unwrap();
                    entered.send(()).unwrap();
                    std::future::pending::<Result<u64, AppError>>().await
                })
            })
            .await
    });
    observed.await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(601)).await;
    assert_eq!(
        call.await.unwrap().unwrap_err(),
        crate::error::TelegramError::Deadline.into()
    );
    assert!(records(&db).is_empty());
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
    store.shutdown().await.unwrap();
}

#[test]
fn shutdown_drains_a_pending_blocking_write_before_unlinking_or_releasing_capacity() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(async {
            let fixture = Fixture::new();
            let db = fixture.database();
            let store = fixture.store(db.clone());
            let (release, blocked) = std::sync::mpsc::channel();
            let (entered, observed) = oneshot::channel();
            let interrupted = CancellationToken::new();
            let writing = interrupted.clone();
            let caller = store.clone();
            let call = tokio::spawn(async move {
                caller
                    .materialize(move |file, _, _| {
                        Box::pin(async move {
                            let _writing = writing.drop_guard();
                            let (started, ready) = oneshot::channel();
                            let _blocker = tokio::task::spawn_blocking(move || {
                                started.send(()).unwrap();
                                let _ = blocked.recv();
                            });
                            ready.await.unwrap();
                            // Queue the write behind the occupied blocking thread, then abandon
                            // its future. Its filesystem operation still belongs to the store.
                            {
                                let writing = file.write_all(b"pending write");
                                tokio::pin!(writing);
                                tokio::select! { biased;
                                    result = &mut writing => result.unwrap(),
                                    _ = std::future::ready(()) => (),
                                }
                            }
                            entered.send(()).unwrap();
                            std::future::pending::<Result<u64, AppError>>().await
                        })
                    })
                    .await
            });
            observed.await.unwrap();
            let closing = store.clone();
            let shutdown = tokio::spawn(async move { closing.shutdown().await });
            interrupted.cancelled().await;
            assert!(!shutdown.is_finished());
            assert_eq!(records(&db).len(), 1);
            assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 1);
            release.send(()).unwrap();
            shutdown.await.unwrap().unwrap();
            assert_eq!(call.await.unwrap().unwrap_err(), AppError::ServiceStopped);
            assert!(records(&db).is_empty());
            assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
        });
}

#[tokio::test]
async fn writer_panic_signals_supervision_after_draining_and_retiring_owned_output() {
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let mut maintenance = Box::pin(store.run_maintenance());
    tokio::select! { biased;
        result = &mut maintenance => panic!("maintenance ended before work: {result:?}"),
        _ = std::future::ready(()) => (),
    }
    let result = store
        .materialize(|file, _, _| {
            Box::pin(async move {
                file.write_all(b"partial").await.unwrap();
                panic!("synthetic writer panic");
            })
        })
        .await;
    assert!(matches!(result, Err(AppError::ServiceStopped)));
    assert_eq!(maintenance.await.unwrap_err(), AppError::ServiceStopped);
    assert!(records(&db).is_empty());
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 0);
    assert_eq!(
        store.shutdown().await.unwrap_err(),
        AppError::ServiceStopped
    );
}

#[tokio::test]
async fn one_maintenance_owner_retires_expired_files_and_shutdown_drains_its_work() {
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let output = save(&store, b"retained").await.unwrap();
    tokio::time::pause();
    let mut maintenance = Box::pin(store.run_maintenance());
    tokio::select! { biased;
        result = &mut maintenance => panic!("maintenance ended before its first tick: {result:?}"),
        _ = std::future::ready(()) => (),
    }
    assert_eq!(
        store.run_maintenance().await.unwrap_err(),
        AppError::DownloadCapacity
    );
    fixture.advance();
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::select! { biased;
        result = &mut maintenance => panic!("maintenance ended before shutdown: {result:?}"),
        _ = std::future::ready(()) => (),
    }
    store.shutdown().await.unwrap();
    maintenance.await.unwrap();
    assert!(records(&db).is_empty());
    assert!(!output.path.exists());
}

#[tokio::test]
async fn root_replacement_during_write_cannot_publish_a_replacement_path_or_delete_foreign_output()
{
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let (entered, observed) = oneshot::channel();
    let (finish, finishing) = oneshot::channel();
    let caller = store.clone();
    let call = tokio::spawn(async move {
        caller
            .materialize(move |file, _, _| {
                Box::pin(async move {
                    file.write_all(b"owned").await.unwrap();
                    file.flush().await.unwrap();
                    entered.send(()).unwrap();
                    finishing.await.unwrap();
                    Ok(5)
                })
            })
            .await
    });
    observed.await.unwrap();
    let name = filename(&records(&db)[0].id).unwrap();
    let pinned = fixture.base.join("pinned");
    fs::rename(fixture.root(), &pinned).unwrap();
    DirBuilder::new()
        .mode(0o700)
        .create(fixture.root())
        .unwrap();
    fs::write(fixture.root().join(&name), b"foreign replacement").unwrap();
    finish.send(()).unwrap();
    assert_eq!(
        call.await.unwrap().unwrap_err(),
        AppError::DownloadStorageUnavailable
    );
    assert_eq!(
        fs::read(fixture.root().join(&name)).unwrap(),
        b"foreign replacement"
    );
    assert!(!pinned.join(&name).exists());
    assert!(records(&db).is_empty());
    assert_eq!(
        save(&store, b"new").await.unwrap_err(),
        AppError::DownloadStorageUnavailable
    );
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn foreign_symlinks_hard_links_and_exclusive_name_collisions_are_never_removed_or_overwritten()
 {
    for linked in [false, true] {
        let fixture = Fixture::new();
        let db = fixture.database();
        let store = fixture.store(db.clone());
        let output = save(&store, b"owned").await.unwrap();
        let retained = fixture.base.join("retained");
        fs::rename(&output.path, &retained).unwrap();
        let outside = fixture.base.join("outside");
        fs::write(&outside, b"foreign source").unwrap();
        if linked {
            fs::hard_link(&outside, &output.path).unwrap();
        } else {
            symlink(&outside, &output.path).unwrap();
        }
        fixture.advance();
        assert_eq!(
            store.cleanup(false).unwrap_err(),
            AppError::DownloadStorageUnavailable
        );
        assert_eq!(fs::read(&outside).unwrap(), b"foreign source");
        assert_eq!(fs::read(&retained).unwrap(), b"owned");
        assert_eq!(records(&db).len(), 1);
        fs::remove_file(&output.path).unwrap();
        fs::rename(&retained, &output.path).unwrap();
        store.cleanup(false).unwrap();
        store.shutdown().await.unwrap();
    }
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let root = &store.0.roots[0];
    let record = db
        .lock()
        .unwrap()
        .reserve_download(
            &root.identity,
            &fixture.config.downloads,
            1_000,
            u64::MAX,
            4_096,
            u64::MAX,
        )
        .unwrap();
    let temporary = root.temporary().unwrap();
    let file = identity(&temporary.metadata().unwrap());
    db.lock()
        .unwrap()
        .own_download_inode(&record.id, &file)
        .unwrap();
    let destination = root.path.join(filename(&record.id).unwrap());
    fs::write(&destination, b"preexisting source").unwrap();
    assert_eq!(
        root.publish(&record, &temporary).unwrap_err(),
        AppError::DownloadStorageUnavailable
    );
    drop(temporary);
    assert_eq!(
        store.cleanup(true).unwrap_err(),
        AppError::DownloadStorageUnavailable
    );
    assert_eq!(fs::read(&destination).unwrap(), b"preexisting source");
    fs::remove_file(destination).unwrap();
    store.cleanup(true).unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_recovers_each_publication_phase_and_preserves_live_files_and_unknown_names() {
    let fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    let completed = save(&store, b"complete").await.unwrap();
    fs::write(fixture.root().join("unknown.bin"), b"unowned").unwrap();
    let root = &store.0.roots[0];
    for phase in 0..4 {
        let record = db
            .lock()
            .unwrap()
            .reserve_download(
                &root.identity,
                &fixture.config.downloads,
                1_000,
                u64::MAX,
                4_096,
                u64::MAX,
            )
            .unwrap();
        if phase > 0 {
            let mut temporary = root.temporary().unwrap();
            let file = identity(&temporary.metadata().unwrap());
            db.lock()
                .unwrap()
                .own_download_inode(&record.id, &file)
                .unwrap();
            if phase > 1 {
                Root::allocate(&temporary, 1_024).unwrap();
                root.publish(&record, &temporary).unwrap();
                temporary.write_all(b"partial").unwrap();
                temporary.sync_all().unwrap();
            }
            if phase == 3 {
                db.lock().unwrap().retire_download(&record.id).unwrap();
                let directory = OpenOptions::new().read(true).open(fixture.root()).unwrap();
                renameat_with(
                    &directory,
                    filename(&record.id).unwrap(),
                    &directory,
                    format!(".retiring-{}", record.id),
                    RenameFlags::NOREPLACE,
                )
                .unwrap();
                directory.sync_all().unwrap();
            }
        }
    }
    drop(store);
    drop(db);
    let db = fixture.database();
    let reopened = fixture.store(db.clone());
    assert_eq!(records(&db).len(), 1);
    assert_eq!(fs::read(&completed.path).unwrap(), b"complete");
    assert_eq!(
        fs::read(fixture.root().join("unknown.bin")).unwrap(),
        b"unowned"
    );
    assert_eq!(fs::read_dir(fixture.root()).unwrap().count(), 2);
    fixture.advance();
    reopened.cleanup(false).unwrap();
    assert!(records(&db).is_empty());
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn root_validation_and_lifetime_locks_fail_closed_without_repairing_permissions() {
    let mut fixture = Fixture::new();
    let db = fixture.database();
    let store = fixture.store(db.clone());
    assert!(DownloadStore::new(db.clone(), &fixture.config).is_err());
    store.shutdown().await.unwrap();
    drop(store);
    fs::set_permissions(fixture.root(), Permissions::from_mode(0o755)).unwrap();
    assert!(DownloadStore::new(db.clone(), &fixture.config).is_err());
    assert_eq!(fs::metadata(fixture.root()).unwrap().mode() & 0o777, 0o755);
    fs::set_permissions(fixture.root(), Permissions::from_mode(0o700)).unwrap();
    let alias = fixture.base.join("alias");
    symlink(fixture.root(), &alias).unwrap();
    fixture.config.downloads.roots = vec![alias];
    assert!(DownloadStore::new(db.clone(), &fixture.config).is_err());
    let alias = fixture.base.join("parent-alias");
    symlink(&fixture.base, &alias).unwrap();
    fixture.config.downloads.roots = vec![alias.join("downloads")];
    assert!(DownloadStore::new(db.clone(), &fixture.config).is_err());
    fixture.config.downloads = Downloads::default();
    let disabled = DownloadStore::new(db.clone(), &fixture.config).unwrap();
    assert_eq!(
        save(&disabled, b"disabled").await.unwrap_err(),
        AppError::DownloadDisabled
    );
    assert!(records(&db).is_empty());
    disabled.shutdown().await.unwrap();
}
