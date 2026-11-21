use super::*;
use crate::config::{CLOUD_DOWNLOAD_BYTES, Config};
use serde_json::{Value, json};
use std::{
    fs, io,
    os::unix::fs::symlink,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::{
    io::AsyncReadExt,
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::super::tests::PrivateDirectory;

const MAX_DOWNLOAD_TIME: Duration = Duration::from_secs(600);
const MAX_LOCAL_DOWNLOAD_TIME: Duration = Duration::from_secs(3_600);

enum Step {
    Reply(Vec<u8>),
    HeldReply(Vec<u8>, oneshot::Receiver<()>),
    Stream(u64),
    Stall(Vec<u8>, oneshot::Sender<()>),
}

fn reply(status: u16, body: &[u8]) -> Step {
    Step::Reply(response(status, body))
}

fn response(status: u16, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn metadata(size: Option<u64>) -> Step {
    file_result(file_metadata(size))
}

fn file_metadata(size: Option<u64>) -> Value {
    let mut file = json!({"file_id":"retained-id","file_unique_id":"unique-id",
        "file_path":"documents/file_1.bin"});
    if let Some(size) = size {
        file["file_size"] = size.into();
    }
    file
}

fn file_result(file: Value) -> Step {
    Step::Reply(file_response(file))
}

fn file_response(file: Value) -> Vec<u8> {
    response(200, json!({"ok":true,"result":file}).to_string().as_bytes())
}

#[derive(Debug)]
struct Request {
    method: String,
    target: String,
    payload: Value,
}

struct Mock {
    client: TelegramClient,
    requests: Arc<Mutex<Vec<Request>>>,
    seen: mpsc::Receiver<()>,
    task: JoinHandle<()>,
}

impl Mock {
    async fn new(steps: Vec<Step>) -> Self {
        Self::with_config(&Config::default(), steps).await
    }

    async fn local(root: &Path, steps: Vec<Step>) -> Self {
        let mut config = Config::default();
        config.telegram.bot_api = BotApi::Local;
        config.telegram.local_bot_api_files_root = Some(root.to_owned());
        Self::with_config(&config, steps).await
    }

    async fn with_config(config: &Config, steps: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TelegramClient::for_test(
            config,
            format!(
                "http://{}/bot123:synthetic/",
                listener.local_addr().unwrap()
            ),
        )
        .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let (seen, received) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            for step in steps {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut input = Vec::new();
                let request = loop {
                    let mut chunk = [0; 1_024];
                    let read = socket.read(&mut chunk).await.unwrap();
                    assert!(read > 0 && input.len() + read <= 8_192);
                    input.extend_from_slice(&chunk[..read]);
                    let Some(end) = input.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = std::str::from_utf8(&input[..end]).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if input.len() != end + 4 + length {
                        continue;
                    }
                    let mut start = headers.lines().next().unwrap().split_whitespace();
                    break Request {
                        method: start.next().unwrap().to_owned(),
                        target: start.next().unwrap().to_owned(),
                        payload: if length == 0 {
                            Value::Null
                        } else {
                            serde_json::from_slice(&input[end + 4..]).unwrap()
                        },
                    };
                };
                recorded.lock().unwrap().push(request);
                match step {
                    Step::Reply(bytes) => {
                        let _ = socket.write_all(&bytes).await;
                        let _ = seen.send(()).await;
                    }
                    Step::HeldReply(bytes, release) => {
                        let _ = seen.send(()).await;
                        if release.await.is_ok() {
                            let _ = socket.write_all(&bytes).await;
                        }
                    }
                    Step::Stream(mut remaining) => {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
                        let _ = seen.send(()).await;
                        let bytes = [b'x'; WRITE_CHUNK_BYTES];
                        while remaining > 0 {
                            let size = remaining.min(bytes.len() as u64) as usize;
                            if socket
                                .write_all(format!("{size:x}\r\n").as_bytes())
                                .await
                                .is_err()
                                || socket.write_all(&bytes[..size]).await.is_err()
                                || socket.write_all(b"\r\n").await.is_err()
                            {
                                break;
                            }
                            remaining -= size as u64;
                        }
                        if remaining == 0 {
                            let _ = socket.write_all(b"0\r\n\r\n").await;
                        }
                    }
                    Step::Stall(prefix, closed) => {
                        socket.write_all(&prefix).await.unwrap();
                        let _ = seen.send(()).await;
                        let mut byte = [0];
                        assert!(!matches!(socket.read(&mut byte).await, Ok(1..)));
                        let _ = closed.send(());
                    }
                }
            }
        });
        Self {
            client,
            requests,
            seen: received,
            task,
        }
    }

    async fn download<W: AsyncWrite + Unpin>(
        &self,
        limit: u64,
        output: &mut W,
    ) -> Result<u64, AppError> {
        self.client
            .download_file(
                "retained-id",
                "unique-id",
                limit,
                MAX_DOWNLOAD_TIME,
                output,
                &TaskTracker::new(),
            )
            .await
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
struct Counter {
    bytes: u64,
    largest_chunk: usize,
}

impl AsyncWrite for Counter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        assert!(bytes.len() <= WRITE_CHUNK_BYTES);
        self.largest_chunk = self.largest_chunk.max(bytes.len());
        self.bytes += bytes.len() as u64;
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn metadata_then_get_returns_actual_bytes_without_trusting_missing_or_false_sizes() {
    for (size, returned_id) in [
        (None, "retained-id"),
        (Some(1), "renewed-id"),
        (Some(9), "retained-id"),
    ] {
        let mut file = file_metadata(size);
        file["file_id"] = returned_id.into();
        let mock = Mock::new(vec![file_result(file), reply(200, b"literal bytes")]).await;
        let mut output = Vec::new();
        assert_eq!(mock.download(13, &mut output).await.unwrap(), 13);
        assert_eq!(output, b"literal bytes");
        let requests = mock.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].target, "/bot123:synthetic/getFile");
        assert_eq!(requests[0].payload, json!({"file_id":"retained-id"}));
        assert_eq!(requests[1].method, "GET");
        assert_eq!(
            requests[1].target,
            "/file/bot123:synthetic/documents/file_1.bin"
        );
        assert_eq!(requests[1].payload, Value::Null);
        assert!(!format!("{:?}", mock.client).contains("synthetic"));
    }
}

#[tokio::test]
async fn lower_limits_and_hard_cloud_ceiling_bound_actual_chunked_bytes() {
    for declared in [None, Some(1)] {
        let mock = Mock::new(vec![metadata(declared), Step::Stream(4)]).await;
        let mut output = Counter::default();
        assert!(matches!(
            mock.download(3, &mut output).await,
            Err(AppError::FileTooLarge { limit_bytes: 3, .. })
        ));
        assert!(output.bytes <= 3);
    }
    for bytes in [CLOUD_DOWNLOAD_BYTES, CLOUD_DOWNLOAD_BYTES + 1] {
        let mock = Mock::new(vec![metadata(None), Step::Stream(bytes)]).await;
        let mut output = Counter::default();
        let result = mock.download(50_000_000, &mut output).await;
        if bytes == CLOUD_DOWNLOAD_BYTES {
            assert_eq!(result.unwrap(), bytes);
            assert_eq!(output.bytes, bytes);
        } else {
            assert!(matches!(
                result,
                Err(AppError::FileTooLarge {
                    limit_bytes: CLOUD_DOWNLOAD_BYTES,
                    ..
                })
            ));
            assert!(output.bytes <= CLOUD_DOWNLOAD_BYTES);
        }
    }
}

#[tokio::test]
async fn declared_oversize_metadata_or_http_headers_fail_before_writes() {
    for (steps, expected_requests) in [
        (vec![metadata(Some(4))], 1),
        (vec![metadata(None), reply(200, b"four")], 2),
    ] {
        let mock = Mock::new(steps).await;
        let mut output = Vec::new();
        assert_eq!(
            mock.download(3, &mut output).await.unwrap_err(),
            AppError::FileTooLarge {
                limit_bytes: 3,
                actual_bytes: 4
            }
        );
        assert!(output.is_empty());
        assert_eq!(mock.count(), expected_requests);
    }
}

#[tokio::test]
async fn malformed_identity_and_unsafe_metadata_never_start_a_file_request() {
    for file in [
        json!({}),
        json!({"file_id":"","file_unique_id":"unique-id","file_path":"safe.bin"}),
        json!({"file_id":"x".repeat(4_097),"file_unique_id":"unique-id","file_path":"safe.bin"}),
        json!({"file_id":"retained-id","file_unique_id":"other","file_path":"safe.bin"}),
        json!({"file_id":"retained-id","file_unique_id":"unique-id","file_size":-1,"file_path":"safe.bin"}),
        json!({"file_id":"retained-id","file_unique_id":"unique-id","file_path":"../private"}),
        json!({"file_id":"retained-id","file_unique_id":"unique-id","file_path":"documents/file?query"}),
        json!({"file_id":"retained-id","file_unique_id":"unique-id","file_path":"documents/%2ffile"}),
    ] {
        let mock = Mock::new(vec![file_result(file)]).await;
        let mut output = Vec::new();
        assert_eq!(
            mock.download(100, &mut output).await.unwrap_err(),
            TelegramError::InvalidResponse.into()
        );
        assert!(output.is_empty());
        assert_eq!(mock.count(), 1);
    }
    let mut mock = Mock::new(vec![metadata(None)]).await;
    mock.client.response_bytes = 32;
    assert_eq!(
        mock.download(100, &mut Vec::new()).await.unwrap_err(),
        TelegramError::ResponseTooLarge.into()
    );
}

#[tokio::test]
async fn unavailable_paths_rejections_and_file_expiry_have_safe_errors() {
    for steps in [
        vec![file_result(
            json!({"file_id":"retained-id","file_unique_id":"unique-id"}),
        )],
        vec![reply(
            400,
            br#"{"ok":false,"error_code":400,"description":"private diagnostic"}"#,
        )],
        vec![
            metadata(None),
            reply(404, b"private expired URL or contents"),
        ],
        vec![metadata(None), reply(410, b"private gone URL or contents")],
    ] {
        let mock = Mock::new(steps).await;
        let mut output = Vec::new();
        let error = mock.download(100, &mut output).await.unwrap_err();
        assert_eq!(error, AppError::DownloadUnavailable);
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains("private"));
        assert!(!diagnostic.contains("synthetic"));
        assert!(std::error::Error::source(&error).is_none());
        assert!(output.is_empty());
    }
}

#[tokio::test]
async fn redirects_partial_responses_and_encoding_are_rejected_without_body_output() {
    for response in [
        b"HTTP/1.1 302 Redirect\r\nLocation: http://127.0.0.1:9/private-token\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 206 Partial\r\nContent-Length: 1\r\n\r\nx".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\n\r\nx".to_vec(),
    ] {
        let mock = Mock::new(vec![metadata(None), Step::Reply(response)]).await;
        let mut output = Vec::new();
        let error = mock.download(100, &mut output).await.unwrap_err();
        assert_eq!(error, TelegramError::InvalidResponse.into());
        assert!(!format!("{error:?} {error}").contains("private-token"));
        assert_eq!(mock.count(), 2);
        assert!(output.is_empty());
    }
    let mock = Mock::new(vec![Step::Reply(b"HTTP/1.1 302 Redirect\r\nLocation: http://127.0.0.1:9/private-token\r\nContent-Length: 0\r\n\r\n".to_vec())]).await;
    assert_eq!(
        mock.download(100, &mut Vec::new()).await.unwrap_err(),
        TelegramError::InvalidResponse.into()
    );
    assert_eq!(mock.count(), 1);
}

struct BrokenWriter {
    fail_on_flush: bool,
}

impl AsyncWrite for BrokenWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.fail_on_flush {
            Poll::Ready(Ok(bytes.len()))
        } else {
            Poll::Ready(Err(io::Error::other(
                "private output path and OS diagnostic",
            )))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other(
            "private output path and OS diagnostic",
        )))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

#[tokio::test]
async fn truncated_bodies_and_writer_failures_are_not_success_or_automatic_replay() {
    let mock = Mock::new(vec![
        metadata(None),
        Step::Reply(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nx".to_vec()),
    ])
    .await;
    assert_eq!(
        mock.download(100, &mut Vec::new()).await.unwrap_err(),
        TelegramError::Transport.into()
    );
    assert_eq!(mock.count(), 2);
    for fail_on_flush in [false, true] {
        let mock = Mock::new(vec![metadata(None), reply(200, b"x")]).await;
        let error = mock
            .download(100, &mut BrokenWriter { fail_on_flush })
            .await
            .unwrap_err();
        assert_eq!(error, AppError::DownloadWriteFailed);
        assert!(!format!("{error:?} {error}").contains("private output path"));
        assert!(std::error::Error::source(&error).is_none());
        assert_eq!(mock.count(), 2);
    }
}

struct BlockedWriter(Option<oneshot::Sender<()>>);

impl AsyncWrite for BlockedWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Some(entered) = self.0.take() {
            let _ = entered.send(());
        }
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

#[tokio::test]
async fn total_deadline_includes_output_backpressure() {
    let mock = Mock::new(vec![metadata(None), reply(200, b"x")]).await;
    let client = mock.client.clone();
    let (entered, observed) = oneshot::channel();
    let call = tokio::spawn(async move {
        client
            .download_file(
                "retained-id",
                "unique-id",
                100,
                Duration::from_secs(2),
                &mut BlockedWriter(Some(entered)),
                &TaskTracker::new(),
            )
            .await
    });
    observed.await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    assert_eq!(
        call.await.unwrap().unwrap_err(),
        TelegramError::Deadline.into()
    );
    assert_eq!(mock.count(), 2);
}

#[tokio::test]
async fn invalid_retained_identifiers_or_bounds_have_no_http_effects() {
    let mock = Mock::new(Vec::new()).await;
    for (id, unique) in [
        ("", "unique-id"),
        ("retained\0id", "unique-id"),
        ("retained-id", ""),
    ] {
        assert_eq!(
            mock.client
                .download_file(
                    id,
                    unique,
                    100,
                    MAX_DOWNLOAD_TIME,
                    &mut Vec::new(),
                    &TaskTracker::new(),
                )
                .await
                .unwrap_err()
                .code(),
            "REQUEST_INVALID"
        );
    }
    for (limit, deadline) in [
        (100, Duration::ZERO),
        (100, MAX_DOWNLOAD_TIME + Duration::from_secs(1)),
    ] {
        assert_eq!(
            mock.client
                .download_file(
                    "retained-id",
                    "unique-id",
                    limit,
                    deadline,
                    &mut Vec::new(),
                    &TaskTracker::new(),
                )
                .await
                .unwrap_err()
                .code(),
            "CONFIG_INVALID"
        );
    }
    assert_eq!(mock.count(), 0);
}

#[tokio::test]
async fn cancellation_and_total_deadline_close_stalled_metadata_or_download_reads() {
    for metadata_phase in [true, false] {
        for cancel in [true, false] {
            let (closed, observed_closed) = oneshot::channel();
            let stall = Step::Stall(
                b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n".to_vec(),
                closed,
            );
            let steps = if metadata_phase {
                vec![stall]
            } else {
                vec![metadata(None), stall]
            };
            let mut mock = Mock::new(steps).await;
            let client = mock.client.clone();
            let call = tokio::spawn(async move {
                client
                    .download_file(
                        "retained-id",
                        "unique-id",
                        100,
                        Duration::from_secs(2),
                        &mut Vec::new(),
                        &TaskTracker::new(),
                    )
                    .await
            });
            for _ in 0..if metadata_phase { 1 } else { 2 } {
                mock.seen.recv().await.unwrap();
            }
            if cancel {
                call.abort();
                assert!(call.await.unwrap_err().is_cancelled());
            } else {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(3)).await;
                assert_eq!(
                    call.await.unwrap().unwrap_err(),
                    TelegramError::Deadline.into()
                );
                tokio::time::resume();
            }
            observed_closed.await.unwrap();
            assert_eq!(mock.count(), if metadata_phase { 1 } else { 2 });
        }
    }
}

#[test]
fn cloud_origin_and_path_validation_reject_url_interpretation_and_traversal() {
    for endpoint in [
        "http://api.telegram.org/botSynthetic/",
        "https://api.telegram.org.attacker.invalid/botSynthetic/",
        "https://user@api.telegram.org/botSynthetic/",
        "https://api.telegram.org:444/botSynthetic/",
        "https://api.telegram.org/botSynthetic/?private",
        "https://api.telegram.org/botSynthetic/#private",
        "https://api.telegram.org/a/../botSynthetic/",
    ] {
        assert_eq!(
            file_endpoint(endpoint, EndpointOrigin::Cloud).unwrap_err(),
            AppError::InvalidConfig("Telegram client endpoint")
        );
    }
    assert!(
        file_endpoint(
            "http://192.0.2.1/botSynthetic/",
            EndpointOrigin::LoopbackMock
        )
        .is_err()
    );
    assert!(
        file_endpoint(
            "https://api.telegram.org/bot123:synthetic/",
            EndpointOrigin::Cloud
        )
        .is_ok()
    );
    for path in [
        "",
        "/private",
        "//127.0.0.1/private",
        "https://example.invalid/file",
        "a/../b",
        "a/./b",
        "a//b",
        "a/%2e%2e/b",
        "a/%2fb",
        "a\\b",
        "a?query",
        "a#fragment",
        "a\nb",
        "résumé.txt",
    ] {
        assert!(!valid_path(path), "{path:?}");
    }
    assert!(!valid_path(&"a".repeat(4_097)));
    assert!(valid_path("documents/file_1-test.bin"));
}

#[test]
fn local_file_endpoint_fixes_the_http_address_and_port() {
    let local = || EndpointOrigin::Local("[::1]:80".parse().unwrap());
    assert_eq!(
        file_endpoint("http://[::1]/bot123:synthetic/", local())
            .unwrap()
            .as_str(),
        "http://[::1]/file/bot123:synthetic/"
    );
    for endpoint in [
        "http://[::1]:8081/botSynthetic/",
        "http://127.0.0.1/botSynthetic/",
        "https://[::1]/botSynthetic/",
        "http://[::1]/botSynthetic/?private",
    ] {
        assert_eq!(
            file_endpoint(endpoint, local()).unwrap_err(),
            AppError::InvalidConfig("Telegram client endpoint")
        );
    }
}

fn local_metadata(path: &Path, size: Option<u64>) -> Step {
    file_result(local_file_metadata(path, size))
}

fn local_file_metadata(path: &Path, size: Option<u64>) -> Value {
    let mut file = file_metadata(size);
    file["file_path"] = json!(path.to_str().unwrap());
    file
}

#[test]
fn local_client_requires_an_existing_real_root() {
    let directory = PrivateDirectory::new();
    let missing = directory.0.join("missing");
    let alias = directory.0.join("alias");
    symlink(&directory.0, &alias).unwrap();
    for root in [missing, alias] {
        let mut config = Config::default();
        config.telegram.bot_api = BotApi::Local;
        config.telegram.local_bot_api_files_root = Some(root);
        assert_eq!(
            TelegramClient::for_test(&config, "http://127.0.0.1:1/botSynthetic/".into())
                .unwrap_err(),
            AppError::InvalidConfig(crate::config::LOCAL_BOT_API_FILES_ROOT)
        );
    }
}

#[tokio::test]
async fn local_get_file_rejects_unconfined_paths_special_files_and_size_mismatches() {
    let directory = PrivateDirectory::new();
    let root = directory.0.join("upstream");
    fs::create_dir(&root).unwrap();
    let source = root.join("file");
    fs::write(&source, b"abc").unwrap();
    let alias = root.join("symlink");
    symlink(&source, &alias).unwrap();
    let outside = directory.0.join("upstream-other");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("file"), b"abc").unwrap();
    let fifo = root.join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    let folder = root.join("folder");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("file"), b"abc").unwrap();
    for (path, size) in [
        (Path::new("documents/file").to_owned(), None),
        (Path::new("documents/file").to_owned(), Some(101)),
        (root.join("folder/../file"), None),
        (root.join("folder//file"), None),
        (root.join("folder/./file"), None),
        (root.join("bad\0file"), None),
        (outside.join("file"), None),
        (alias, None),
        (fifo, None),
        (folder, None),
        (source, Some(2)),
    ] {
        let mock = Mock::local(&root, vec![local_metadata(&path, size)]).await;
        let mut output = Counter::default();
        assert_eq!(
            mock.download(100, &mut output).await.unwrap_err(),
            TelegramError::InvalidResponse.into()
        );
        assert_eq!(output.bytes, 0);
        assert_eq!(mock.count(), 1);
    }
}

#[tokio::test]
async fn local_get_file_rejects_a_replaced_upstream_root() {
    let directory = PrivateDirectory::new();
    let root = directory.0.join("upstream");
    fs::create_dir(&root).unwrap();
    let source = root.join("file");
    fs::write(&source, b"approved").unwrap();
    let mock = Mock::local(&root, vec![local_metadata(&source, None)]).await;
    fs::rename(&root, directory.0.join("moved")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(&source, b"substitute").unwrap();
    let mut output = Vec::new();
    assert_eq!(
        mock.download(100, &mut output).await.unwrap_err(),
        TelegramError::InvalidResponse.into()
    );
    assert!(output.is_empty());
}

#[tokio::test]
async fn local_copy_checks_sparse_size_before_writing_and_streams_in_bounded_chunks() {
    let directory = PrivateDirectory::new();
    let source = directory.0.join("123:synthetic").join("file");
    fs::create_dir(source.parent().unwrap()).unwrap();
    let input = fs::File::create(&source).unwrap();
    let limit = 1 << 30;
    input.set_len(limit + 1).unwrap();
    let mock = Mock::local(&directory.0, vec![local_metadata(&source, None)]).await;
    let mut output = Counter::default();
    assert_eq!(
        mock.download(limit, &mut output).await.unwrap_err(),
        AppError::FileTooLarge {
            limit_bytes: limit,
            actual_bytes: limit + 1
        }
    );
    assert_eq!(output.bytes, 0);

    let size = 3 * (1 << 20) + 17;
    input.set_len(size).unwrap();
    let mock = Mock::local(&directory.0, vec![local_metadata(&source, Some(size))]).await;
    assert_eq!(
        mock.client
            .download_file(
                "retained-id",
                "unique-id",
                limit,
                MAX_LOCAL_DOWNLOAD_TIME,
                &mut output,
                &TaskTracker::new(),
            )
            .await
            .unwrap(),
        size
    );
    assert_eq!(output.bytes, size);
    assert_eq!(output.largest_chunk, WRITE_CHUNK_BYTES);
    assert_eq!(fs::metadata(source).unwrap().len(), size);
    assert_eq!(mock.count(), 1);
}

#[tokio::test]
async fn local_get_file_accepts_the_original_response_after_the_request_timeout() {
    let directory = PrivateDirectory::new();
    let source = directory.0.join("file");
    fs::write(&source, b"abc").unwrap();
    let (release, held) = oneshot::channel();
    let file = file_response(local_file_metadata(&source, Some(3)));
    let mut mock = Mock::local(
        &directory.0,
        vec![Step::HeldReply(file, held), reply(400, b"")],
    )
    .await;
    let client = mock.client.clone();
    let call = tokio::spawn(async move {
        let mut output = Vec::new();
        let size = client
            .download_file(
                "retained-id",
                "unique-id",
                100,
                MAX_LOCAL_DOWNLOAD_TIME,
                &mut output,
                &TaskTracker::new(),
            )
            .await?;
        Ok::<_, AppError>((size, output))
    });
    mock.seen.recv().await.unwrap();
    tokio::time::pause();
    tokio::time::sleep(REQUEST_TIMEOUT * 2).await;
    tokio::time::resume();
    release.send(()).unwrap();
    assert_eq!(call.await.unwrap().unwrap(), (3, b"abc".to_vec()));
    assert_eq!(mock.count(), 1);
}

#[tokio::test]
async fn local_get_file_closes_at_the_transfer_deadline_without_retry() {
    use tokio::time::{Instant, sleep_until, timeout_at};

    let directory = PrivateDirectory::new();
    let (closed, mut observed_closed) = oneshot::channel();
    let mut mock = Mock::local(
        &directory.0,
        vec![
            Step::Stall(
                b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n".to_vec(),
                closed,
            ),
            reply(400, b""),
        ],
    )
    .await;
    let client = mock.client.clone();
    let (started, observed_start) = oneshot::channel();
    let mut call = tokio::spawn(async move {
        started.send(Instant::now()).unwrap();
        client
            .download_file(
                "retained-id",
                "unique-id",
                100,
                MAX_LOCAL_DOWNLOAD_TIME,
                &mut Vec::new(),
                &TaskTracker::new(),
            )
            .await
    });
    mock.seen.recv().await.unwrap();
    let deadline = observed_start.await.unwrap() + MAX_LOCAL_DOWNLOAD_TIME;
    tokio::time::pause();
    tokio::select! {
        biased;
        result = &mut call => panic!("getFile completed before the transfer deadline: {result:?}"),
        result = &mut observed_closed => panic!("getFile connection closed early: {result:?}"),
        () = sleep_until(deadline - Duration::from_secs(1)) => (),
    }
    assert_eq!(mock.count(), 1);
    let result = timeout_at(deadline + Duration::from_secs(1), &mut call)
        .await
        .expect("getFile exceeded the transfer deadline")
        .unwrap();
    assert_eq!(result.unwrap_err(), TelegramError::Deadline.into());
    assert!(Instant::now() >= deadline);
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(5), observed_closed)
        .await
        .expect("getFile connection remained open after the transfer deadline")
        .unwrap();
    assert_eq!(mock.count(), 1);
    assert_eq!(
        mock.client
            .download_file(
                "retained-id",
                "unique-id",
                100,
                MAX_LOCAL_DOWNLOAD_TIME + Duration::from_secs(1),
                &mut Vec::new(),
                &TaskTracker::new(),
            )
            .await
            .unwrap_err(),
        AppError::InvalidConfig("download bounds")
    );
}
