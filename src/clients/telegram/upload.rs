//! Multipart streaming from an already confined descriptor, with no path reopening or copy.

use std::{fs::File, io};

use futures_util::{Stream, stream};
use reqwest::{
    Body,
    multipart::{Form, Part},
};
use tokio::io::AsyncReadExt;

use super::{TelegramClient, send_error, validate_sent};
use crate::{
    clients::telegram::SentMessage,
    error::{AppError, TelegramError},
};

const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
const CHUNK_BYTES: usize = 64 * 1_024;

impl TelegramClient {
    /// The service validates the complete caption, destination and file before admission.
    /// Attempt once; any transport/body failure may follow acceptance by Telegram.
    pub(crate) async fn send_document(
        &self,
        chat_id: i64,
        caption: String,
        file: File,
        filename: String,
        size_bytes: u64,
    ) -> Result<SentMessage, AppError> {
        let part =
            Part::stream_with_length(Body::wrap_stream(file_chunks(file, size_bytes)), size_bytes)
                .file_name(filename);
        let form = Form::new()
            .text("chat_id", chat_id.to_string())
            .text("caption", caption)
            .part("document", part);
        let request = self
            .http
            .post(format!("{}sendDocument", self.endpoint))
            .multipart(form);
        let sent = tokio::time::timeout(UPLOAD_TIMEOUT, self.execute(request))
            .await
            .unwrap_or(Err(TelegramError::Deadline))
            .map_err(send_error)?;
        validate_sent(&sent, chat_id)?;
        Ok(sent)
    }
}

fn file_chunks(file: File, size_bytes: u64) -> impl Stream<Item = io::Result<Vec<u8>>> + Send {
    let mut file = tokio::fs::File::from_std(file);
    // Bound Tokio's blocking-read buffer as well as the chunk passed to the HTTP body.
    file.set_max_buf_size(CHUNK_BYTES);
    stream::try_unfold((file, size_bytes), |(mut file, remaining)| async move {
        if remaining == 0 {
            return Ok(None);
        }
        let mut chunk = vec![0; remaining.min(CHUNK_BYTES as u64) as usize];
        let read = file
            .read(&mut chunk)
            .await
            .map_err(|error| io::Error::from(error.kind()))?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        chunk.truncate(read);
        Ok(Some((chunk, (file, remaining - read as u64))))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::TryStreamExt;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt},
        path::PathBuf,
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let mut random = [0; 16];
            getrandom::fill(&mut random).unwrap();
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("upload-test-{:032x}", u128::from_ne_bytes(random)));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }

        fn file(&self, size: u64) -> File {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(self.0.join("document.bin"))
                .unwrap();
            file.set_len(size).unwrap();
            file
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[tokio::test]
    async fn near_limit_stream_is_lazy_chunk_bounded_and_cannot_follow_growth() {
        let fixture = Fixture::new();
        let size = 50_000_000;
        let writer = fixture.file(size);
        let mut chunks = Box::pin(file_chunks(writer.try_clone().unwrap(), size));
        let first = chunks.try_next().await.unwrap().unwrap();
        assert!(!first.is_empty() && first.len() <= CHUNK_BYTES);
        // Mutate only after consumption starts: unread content is not an in-memory snapshot.
        writer.set_len(size + 1_000_000).unwrap();
        writer.write_all_at(b"x", first.len() as u64).unwrap();
        let second = chunks.try_next().await.unwrap().unwrap();
        assert_eq!(second[0], b'x');
        let mut total = (first.len() + second.len()) as u64;
        while let Some(chunk) = chunks.try_next().await.unwrap() {
            assert!(!chunk.is_empty() && chunk.len() <= CHUNK_BYTES);
            total += chunk.len() as u64;
        }
        assert_eq!(total, size);
    }

    #[tokio::test]
    async fn truncation_errors_instead_of_completing_a_partial_document() {
        let fixture = Fixture::new();
        let writer = fixture.file(CHUNK_BYTES as u64 * 2);
        let mut chunks = Box::pin(file_chunks(
            writer.try_clone().unwrap(),
            CHUNK_BYTES as u64 * 2,
        ));
        let first = chunks.try_next().await.unwrap().unwrap();
        writer.set_len(first.len() as u64).unwrap();
        assert_eq!(
            chunks.try_next().await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert!(chunks.try_next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_failure_terminates_the_stream_without_exposing_the_source_path() {
        let fixture = Fixture::new();
        let _file = fixture.file(1);
        // A write-only descriptor supplies a deterministic OS read error without chmod races.
        let file = fs::OpenOptions::new()
            .write(true)
            .open(fixture.0.join("document.bin"))
            .unwrap();
        let mut chunks = Box::pin(file_chunks(file, 1));
        let error = chunks.try_next().await.unwrap_err();
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains("document.bin"));
        assert!(!diagnostic.contains(fixture.0.to_str().unwrap()));
        assert!(chunks.try_next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn empty_snapshot_never_reads_later_growth() {
        let fixture = Fixture::new();
        let writer = fixture.file(0);
        let mut chunks = Box::pin(file_chunks(writer.try_clone().unwrap(), 0));
        writer.write_all_at(b"later", 0).unwrap();
        assert!(chunks.try_next().await.unwrap().is_none());
    }
}
