//! Private bounded download storage with caller-supplied current authorization.

use futures_util::FutureExt;
use std::{
    future::Future,
    os::unix::fs::MetadataExt,
    panic::AssertUnwindSafe,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    fs::File,
    sync::{Semaphore, oneshot},
    time::{Instant, MissedTickBehavior},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use super::unix_millis;
use crate::{
    clients::telegram::TelegramClient,
    config::{Config, Downloads},
    error::AppError,
    repositories::{Database, DownloadRecord, FileIdentity},
};

mod files;
use files::{Root, filename, identity};

type Clock = Arc<dyn Fn() -> Result<i64, AppError> + Send + Sync>;
type WriteFuture<'a> = Pin<Box<dyn Future<Output = Result<u64, AppError>> + Send + 'a>>;
type Authorization = Arc<dyn Fn() -> Result<(), AppError> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct DownloadStore(Arc<Inner>);

struct Inner {
    database: Arc<Mutex<Database>>,
    policy: Downloads,
    roots: Vec<Root>,
    admission: Arc<Semaphore>,
    maintenance: Semaphore,
    lifecycle: Mutex<bool>,
    retirement: Mutex<()>,
    stop: CancellationToken,
    failed: CancellationToken,
    tasks: TaskTracker,
    now: Clock,
}

pub(crate) struct DownloadedFile {
    pub id: String,
    pub path: PathBuf,
    pub identity: FileIdentity,
    pub bytes: u64,
    pub expires_at: i64,
}

impl std::fmt::Debug for DownloadStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DownloadStore([REDACTED])")
    }
}
impl std::fmt::Debug for DownloadedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DownloadedFile([REDACTED])")
    }
}

impl DownloadStore {
    pub fn new(database: Arc<Mutex<Database>>, config: &Config) -> Result<Self, AppError> {
        Self::build(database, config, Arc::new(unix_millis))
    }

    fn build(
        database: Arc<Mutex<Database>>,
        config: &Config,
        now: Clock,
    ) -> Result<Self, AppError> {
        let mut policy = config.downloads.clone();
        policy.normalize(
            None,
            &config.files,
            &config.storage,
            [
                config.server.auth_token_file.as_deref(),
                config.telegram.bot_token_file.as_deref(),
            ],
        )?;
        let roots = policy
            .roots
            .iter()
            .map(|path| Root::new(path))
            .collect::<Result<Vec<_>, _>>()?;
        let store = Self(Arc::new(Inner {
            admission: Arc::new(Semaphore::new(policy.concurrent as usize)),
            database,
            policy,
            roots,
            maintenance: Semaphore::new(1),
            lifecycle: Mutex::new(false),
            retirement: Mutex::new(()),
            stop: CancellationToken::new(),
            failed: CancellationToken::new(),
            tasks: TaskTracker::new(),
            now,
        }));
        // Refuse to forget ownership beneath a removed/replaced/unconfigured root.
        store.visit(|record| store.root(&record.root).map(|_| ()))?;
        store.cleanup(true)?;
        Ok(store)
    }

    /// The caller must first revalidate retained attachment provenance and current authority.
    pub async fn download(
        &self,
        telegram: TelegramClient,
        file_id: String,
        unique_id: String,
        authorize: impl Fn() -> Result<(), AppError> + Send + Sync + 'static,
    ) -> Result<DownloadedFile, AppError> {
        self.materialize_authorized(
            move |file, limit, timeout| {
                Box::pin(async move {
                    telegram
                        .download_file(&file_id, &unique_id, limit, timeout, file)
                        .await
                })
            },
            Arc::new(authorize),
        )
        .await
    }

    #[cfg(test)]
    async fn materialize<W>(&self, write: W) -> Result<DownloadedFile, AppError>
    where
        W: for<'a> FnOnce(&'a mut File, u64, Duration) -> WriteFuture<'a> + Send + 'static,
    {
        self.materialize_authorized(write, Arc::new(|| Ok(())))
            .await
    }

    async fn materialize_authorized<W>(
        &self,
        write: W,
        authorize: Authorization,
    ) -> Result<DownloadedFile, AppError>
    where
        W: for<'a> FnOnce(&'a mut File, u64, Duration) -> WriteFuture<'a> + Send + 'static,
    {
        if !self.0.policy.enabled {
            return Err(AppError::DownloadDisabled);
        }
        let deadline = Instant::now() + Duration::from_secs(self.0.policy.timeout_seconds);
        let permit = self.0.admission.clone().try_acquire_owned().map_err(|_| {
            if self.0.stop.is_cancelled() {
                AppError::ServiceStopped
            } else {
                AppError::DownloadCapacity
            }
        })?;
        let cancel = CancellationToken::new();
        let _cancel = cancel.clone().drop_guard();
        let (send, receive) = oneshot::channel();
        {
            // Registration and close are serialized: shutdown cannot miss an accepted job.
            let closed = self
                .0
                .lifecycle
                .lock()
                .map_err(|_| AppError::StorageFailure)?;
            if *closed {
                return Err(AppError::ServiceStopped);
            }
            let store = self.clone();
            let outcome = DownloadOutcome(Some(self.0.failed.clone()));
            self.0.tasks.spawn(async move {
                let _permit = permit;
                let result = store
                    .write_owned(write, cancel, deadline, authorize)
                    .await
                    .map(|file| DownloadHandoff {
                        store: store.clone(),
                        file: Some(file),
                    });
                outcome.complete();
                let _ = send.send(result);
            });
        }
        let pending = receive.await.map_err(|_| AppError::ServiceStopped)??;
        pending.accept()
    }

    async fn write_owned<W>(
        &self,
        write: W,
        cancel: CancellationToken,
        deadline: Instant,
        authorize: Authorization,
    ) -> Result<DownloadedFile, AppError>
    where
        W: for<'a> FnOnce(&'a mut File, u64, Duration) -> WriteFuture<'a> + Send + 'static,
    {
        self.ensure_running(&cancel, deadline)?;
        authorize()?;
        let record = self.reserve()?;
        let result = self
            .write_record(&record, write, &cancel, deadline, &authorize)
            .await;
        if result.is_err() {
            // Reservations survive cleanup failure and are retried by maintenance/restart.
            self.retire(&record.id)?;
        }
        result
    }

    fn ensure_running(
        &self,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<(), AppError> {
        if cancel.is_cancelled() || self.0.stop.is_cancelled() {
            return Err(AppError::ServiceStopped);
        }
        if Instant::now() >= deadline {
            return Err(crate::error::TelegramError::Deadline.into());
        }
        Ok(())
    }

    fn reserve(&self) -> Result<DownloadRecord, AppError> {
        let now = (self.0.now)()?;
        let mut capacity = false;
        for root in &self.0.roots {
            root.current()?;
            let (available, allocation_unit, available_files) = match root.available() {
                Ok(space) => space,
                Err(AppError::DownloadCapacity) => {
                    capacity = true;
                    continue;
                }
                Err(error) => return Err(error),
            };
            match self
                .0
                .database
                .lock()
                .map_err(|_| AppError::StorageFailure)?
                .reserve_download(
                    &root.identity,
                    &self.0.policy,
                    now,
                    available,
                    allocation_unit,
                    available_files,
                ) {
                Ok(record) => return Ok(record),
                Err(AppError::DownloadCapacity) => capacity = true,
                Err(error) => return Err(error),
            }
        }
        Err(if capacity {
            AppError::DownloadCapacity
        } else {
            AppError::DownloadStorageUnavailable
        })
    }

    async fn write_record<W>(
        &self,
        record: &DownloadRecord,
        write: W,
        cancel: &CancellationToken,
        deadline: Instant,
        authorize: &Authorization,
    ) -> Result<DownloadedFile, AppError>
    where
        W: for<'a> FnOnce(&'a mut File, u64, Duration) -> WriteFuture<'a> + Send + 'static,
    {
        self.ensure_running(cancel, deadline)?;
        authorize()?;
        let root = self.root(&record.root)?;
        let temporary = root.temporary()?;
        let expected = identity(
            &temporary
                .metadata()
                .map_err(|_| AppError::DownloadStorageUnavailable)?,
        );
        self.0
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .own_download_inode(&record.id, &expected)?;
        let bytes = record.reserved_bytes;
        let temporary = tokio::task::spawn_blocking(move || {
            Root::allocate(&temporary, bytes)?;
            temporary
                .sync_all()
                .map_err(|_| AppError::DownloadStorageUnavailable)?;
            Ok::<_, AppError>(temporary)
        })
        .await
        .map_err(|_| AppError::ServiceStopped)??;
        if temporary
            .metadata()
            .map_err(|_| AppError::DownloadStorageUnavailable)?
            .blocks()
            .saturating_mul(512)
            > record.disk_bytes
        {
            return Err(AppError::DownloadCapacity);
        }
        self.ensure_running(cancel, deadline)?;
        authorize()?;
        root.publish(record, &temporary)?;
        self.ensure_running(cancel, deadline)?;
        let mut file = File::from_std(temporary);
        file.set_max_buf_size(65_536);
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(crate::error::TelegramError::Deadline)?;
        let result = AssertUnwindSafe(async { tokio::select! {
            biased;
            _ = self.0.stop.cancelled() => Err(AppError::ServiceStopped),
            _ = cancel.cancelled() => Err(AppError::ServiceStopped),
            result = tokio::time::timeout_at(deadline, async {
                let actual = write(&mut file, bytes, remaining).await?;
                if actual > bytes { return Err(AppError::FileTooLarge { limit_bytes: bytes, actual_bytes: actual }); }
                Ok(actual)
            }) => result.unwrap_or(Err(crate::error::TelegramError::Deadline.into())),
        }}).catch_unwind().await.unwrap_or_else(|_| {
            self.0.failed.cancel();
            self.close();
            Err(AppError::ServiceStopped)
        });
        // into_std waits for Tokio's outstanding blocking-file operation even after a
        // canceled write/flush future. Never unlink or release its reservation earlier.
        let file = file.into_std().await;
        let actual = result?;
        self.ensure_running(cancel, deadline)?;
        // Do not cancel Tokio sync_all: its detached blocking Arc can outlive that future.
        // This owned job is always joined before file cleanup or quota release.
        let file = tokio::task::spawn_blocking(move || {
            file.set_len(actual)
                .map_err(|_| AppError::DownloadWriteFailed)?;
            file.sync_all().map_err(|_| AppError::DownloadWriteFailed)?;
            Ok::<_, AppError>(file)
        })
        .await
        .map_err(|_| AppError::ServiceStopped)??;
        self.ensure_running(cancel, deadline)?;
        root.current()?;
        root.verify(&filename(&record.id)?, &expected)?;
        let metadata = file
            .metadata()
            .map_err(|_| AppError::DownloadStorageUnavailable)?;
        if metadata.len() != actual {
            return Err(AppError::DownloadStorageUnavailable);
        }
        drop(file);
        let completed = self
            .0
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .complete_download(
                &record.id,
                actual,
                metadata.blocks().saturating_mul(512),
                (self.0.now)()?,
                self.0.policy.retention_seconds,
            )?;
        self.ensure_running(cancel, deadline)?;
        Ok(DownloadedFile {
            id: completed.id.clone(),
            path: root.path.join(filename(&record.id)?),
            identity: completed.file.ok_or(AppError::StorageFailure)?,
            bytes: completed.actual_bytes.ok_or(AppError::StorageFailure)?,
            expires_at: completed.expires_at,
        })
    }

    fn root(&self, identity: &FileIdentity) -> Result<&Root, AppError> {
        self.0
            .roots
            .iter()
            .find(|root| root.identity == *identity)
            .ok_or(AppError::DownloadStorageUnavailable)
    }

    /// Recheck the owned identity and durably cap retention by the authorized source expiry.
    pub fn retain_until(&self, id: &str, expiry: i64) -> Result<i64, AppError> {
        let record = self
            .0
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .download_record(id)?
            .ok_or(AppError::AttachmentUnavailable)?;
        let root = self.root(&record.root)?;
        root.current()?;
        root.verify(
            &filename(id)?,
            record
                .file
                .as_ref()
                .ok_or(AppError::AttachmentUnavailable)?,
        )?;
        self.0
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .cap_download_expiry(id, expiry, (self.0.now)()?)
    }

    pub fn discard(&self, id: &str) -> Result<(), AppError> {
        self.retire(id)
    }

    fn visit(
        &self,
        mut visitor: impl FnMut(&DownloadRecord) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let mut cursor = String::new();
        for _ in 0..11 {
            let page = self
                .0
                .database
                .lock()
                .map_err(|_| AppError::StorageFailure)?
                .download_records(&cursor)?;
            let done = page.len() < 100;
            if let Some(last) = page.last() {
                cursor = last.id.clone();
            }
            for record in &page {
                visitor(record)?;
            }
            if done {
                return Ok(());
            }
        }
        Err(AppError::StorageFailure)
    }

    fn retire(&self, id: &str) -> Result<(), AppError> {
        let _retirement = self
            .0
            .retirement
            .lock()
            .map_err(|_| AppError::StorageFailure)?;
        let Some(record) = self
            .0
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .download_record(id)?
        else {
            return Ok(());
        };
        let root = self.root(&record.root)?;
        self.0
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .retire_download(id)?;
        root.remove(&record)?;
        self.0
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .forget_download(id)
    }

    fn cleanup(&self, recover: bool) -> Result<(), AppError> {
        let now = (self.0.now)()?;
        self.visit(|record| {
            if recover && !record.complete
                || record.retiring
                || record.complete && record.expires_at <= now
            {
                self.retire(&record.id)?;
            }
            Ok(())
        })
    }

    /// One bounded maintenance owner supervised by daemon composition.
    pub async fn run_maintenance(&self) -> Result<(), AppError> {
        let _maintenance = self
            .0
            .maintenance
            .try_acquire()
            .map_err(|_| AppError::DownloadCapacity)?;
        let period = Duration::from_secs(self.0.policy.cleanup_interval_seconds);
        let mut ticks = tokio::time::interval_at(Instant::now() + period, period);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! { biased;
            _ = self.0.failed.cancelled() => return Err(AppError::ServiceStopped),
            _ = self.0.stop.cancelled() => return Ok(()), _ = ticks.tick() => () }
            let work = {
                let closed = self
                    .0
                    .lifecycle
                    .lock()
                    .map_err(|_| AppError::StorageFailure)?;
                if *closed {
                    return Ok(());
                }
                let store = self.clone();
                self.0.tasks.spawn_blocking(move || store.cleanup(false))
            };
            work.await.map_err(|_| AppError::ServiceStopped)??;
        }
    }

    pub fn close(&self) {
        if let Ok(mut closed) = self.0.lifecycle.lock() {
            *closed = true;
        } else {
            self.0.failed.cancel();
        }
        self.0.admission.close();
        self.0.stop.cancel();
        self.0.tasks.close();
    }

    pub async fn shutdown(&self) -> Result<(), AppError> {
        self.close();
        self.0.tasks.wait().await;
        let cleanup = self.cleanup(true);
        cleanup.and(if self.0.failed.is_cancelled() {
            Err(AppError::ServiceStopped)
        } else {
            Ok(())
        })
    }
}

// The owned job can finish before its caller is polled again. Keep cleanup ownership
// in the channel until that caller accepts the result, including failed sends.
struct DownloadHandoff {
    store: DownloadStore,
    file: Option<DownloadedFile>,
}
impl DownloadHandoff {
    fn accept(mut self) -> Result<DownloadedFile, AppError> {
        self.file.take().ok_or(AppError::ServiceStopped)
    }
}
impl Drop for DownloadHandoff {
    fn drop(&mut self) {
        if let Some(file) = &self.file
            && self.store.retire(&file.id).is_err()
        {
            // Failed retirement keeps its durable reservation for restart recovery.
            self.store.0.failed.cancel();
            self.store.close();
        }
    }
}

struct DownloadOutcome(Option<CancellationToken>);
impl DownloadOutcome {
    fn complete(mut self) {
        self.0 = None;
    }
}
impl Drop for DownloadOutcome {
    fn drop(&mut self) {
        if let Some(failed) = &self.0 {
            failed.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
