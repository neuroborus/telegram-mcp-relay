//! Current-policy authorization precedes download effects and is rechecked before disclosure.

use std::sync::{Arc, Mutex};

use super::{downloads::DownloadStore, unix_millis};
use crate::{
    clients::telegram::TelegramClient,
    config::Config,
    dto::{GetAttachmentRequest, GetAttachmentResult, HistoryAttachment, LocalFileIdentity},
    error::AppError,
    repositories::{Database, HistoryFile, RetainedAttachment},
};

#[derive(Clone)]
pub(crate) struct AttachmentService {
    database: Arc<Mutex<Database>>,
    telegram: TelegramClient,
    pub store: DownloadStore,
    chats: Vec<i64>,
    users: Vec<u64>,
    shared: bool,
}

impl AttachmentService {
    pub fn new(
        database: Arc<Mutex<Database>>,
        telegram: TelegramClient,
        config: &Config,
    ) -> Result<Self, AppError> {
        let authenticated =
            config.server.auth_token_env.is_some() || config.server.auth_token_file.is_some();
        config
            .history
            .validate(&config.storage, &config.limits, authenticated)?;
        if config.downloads.enabled
            && (config.telegram.allowed_chat_ids.is_empty()
                || config.telegram.allowed_chat_ids.len() > 256
                || config.telegram.allowed_user_ids.is_empty()
                || config.telegram.allowed_user_ids.len() > 256
                || config
                    .telegram
                    .allowed_chat_ids
                    .iter()
                    .any(|id| *id == 0 || id.unsigned_abs() > (1 << 52) - 1)
                || config
                    .telegram
                    .allowed_user_ids
                    .iter()
                    .any(|id| *id == 0 || *id > (1 << 52) - 1))
        {
            return Err(AppError::InvalidConfig("download allowlists"));
        }
        Ok(Self {
            store: DownloadStore::new(database.clone(), config)?,
            database,
            telegram,
            chats: config.telegram.allowed_chat_ids.clone(),
            users: config.telegram.allowed_user_ids.clone(),
            shared: config.history.enabled && authenticated,
        })
    }

    fn authorize(
        &self,
        db: &Database,
        session: &str,
        handle: &str,
        now: i64,
    ) -> Result<RetainedAttachment, AppError> {
        db.session(session)?;
        if now < 0
            || handle.len() != 64
            || !handle
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(AppError::AttachmentUnavailable);
        }
        let mut file = db
            .retained_attachment(session, handle, now)?
            .ok_or(AppError::AttachmentUnavailable)?;
        if !self.chats.contains(&file.source.chat_id)
            || file.source.chat_id.unsigned_abs() > (1 << 52) - 1
            || file.source.message_id <= 0
            || file.source.message_id.unsigned_abs() > (1 << 52) - 1
            || file
                .source
                .sender_user_id
                .is_some_and(|id| id == 0 || id > (1 << 52) - 1)
            || !bounded(&file.file_id, 4096)
            || !bounded(&file.file_unique_id, 512)
            || file
                .display
                .filename
                .as_ref()
                .is_some_and(|s| s.len() > 1024)
            || file
                .display
                .mime_type
                .as_ref()
                .is_some_and(|s| s.len() > 256)
        {
            return Err(AppError::AttachmentUnavailable);
        }
        let user = file
            .source
            .sender_user_id
            .is_some_and(|id| self.users.contains(&id));
        let own = user.then_some(file.own_expires_at).flatten();
        let shared = (self.shared
            && db.history_enabled()
            && match file.direction.as_deref() {
                Some("incoming") => user,
                Some("outgoing" | "service") => file.source.sender_user_id.is_none(),
                _ => false,
            })
        .then_some(file.shared_expires_at)
        .flatten();
        let expiry = own.or(shared).ok_or(AppError::AttachmentUnavailable)?;
        file.display.expires_at = file.display.expires_at.min(expiry);
        Ok(file)
    }

    pub fn own_files(
        &self,
        db: &Database,
        session: &str,
        reply: &str,
        now: i64,
    ) -> Result<Vec<HistoryAttachment>, AppError> {
        let mut result = Vec::new();
        for handle in db.own_attachment_handles(session, reply, now)? {
            match self.authorize(db, session, &handle, now) {
                Ok(file)
                    if file.own_expires_at.is_some()
                        && file
                            .source
                            .sender_user_id
                            .is_some_and(|id| self.users.contains(&id)) =>
                {
                    result.push(file.display.into())
                }
                Ok(_) | Err(AppError::AttachmentUnavailable) => (),
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }

    pub async fn download(
        &self,
        request: GetAttachmentRequest,
    ) -> Result<GetAttachmentResult, AppError> {
        let source = {
            let db = self.database.lock().map_err(|_| AppError::StorageFailure)?;
            self.authorize(
                &db,
                &request.session_key,
                &request.attachment_handle,
                unix_millis()?,
            )?
        };
        let service = self.clone();
        let expected = source.clone();
        let session = request.session_key.clone();
        let handle = request.attachment_handle.clone();
        let mut output = self
            .store
            .download(
                self.telegram.clone(),
                source.file_id.clone(),
                source.file_unique_id.clone(),
                move || {
                    let db = service
                        .database
                        .lock()
                        .map_err(|_| AppError::StorageFailure)?;
                    let current = service.authorize(&db, &session, &handle, unix_millis()?)?;
                    if same_source(&expected, &current) {
                        Ok(())
                    } else {
                        Err(AppError::AttachmentUnavailable)
                    }
                },
            )
            .await?;
        let result = (|| {
            let current = {
                let db = self.database.lock().map_err(|_| AppError::StorageFailure)?;
                self.authorize(
                    &db,
                    &request.session_key,
                    &request.attachment_handle,
                    unix_millis()?,
                )?
            };
            if !same_source(&source, &current) {
                return Err(AppError::AttachmentUnavailable);
            }
            output.expires_at = self
                .store
                .retain_until(&output.id, current.display.expires_at)?;
            Ok(current.display)
        })();
        let metadata = match result {
            Ok(metadata) => metadata,
            Err(error) => {
                self.store.discard(&output.id)?;
                return Err(error);
            }
        };
        Ok(GetAttachmentResult {
            attachment: metadata.into(),
            download_id: output.id,
            path: output.path,
            file_identity: LocalFileIdentity {
                device: output.identity.device,
                inode: output.identity.inode,
            },
            size_bytes: output.bytes,
            expires_at: output.expires_at,
        })
    }
}

fn same_source(first: &RetainedAttachment, second: &RetainedAttachment) -> bool {
    first.file_id == second.file_id
        && first.file_unique_id == second.file_unique_id
        && first.source.chat_id == second.source.chat_id
        && first.source.message_id == second.source.message_id
        && first.source.sender_user_id == second.source.sender_user_id
        && first.display.kind == second.display.kind
}

fn bounded(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

impl From<HistoryFile> for HistoryAttachment {
    fn from(file: HistoryFile) -> Self {
        Self {
            attachment_handle: file.handle,
            kind: file.kind,
            filename: file.filename,
            mime_type: file.mime_type,
            file_size: file.file_size,
            width: file.width,
            height: file.height,
            expires_at: file.expires_at,
        }
    }
}

#[cfg(test)]
mod tests;
