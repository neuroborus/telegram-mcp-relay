use super::*;
use crate::domain::outbound::{SessionTitle, format_text};

fn formatted(body: &str) -> FormattedMessage {
    format_text(&SessionTitle::new("Session"), None, body).unwrap()
}
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
};

const SENT: &str = r#"{"ok":true,"result":{"message_id":7,"date":100,"chat":{"id":101}}}"#;

enum Step {
    Reply(Vec<u8>),
    Disconnect,
    Hang(Vec<u8>),
}

fn reply(status: u16, body: &str) -> Step {
    Step::Reply(
        format!(
            "HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
}

struct Mock {
    client: TelegramClient,
    requests: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    seen: mpsc::Receiver<()>,
    task: JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Mock {
    async fn new(steps: Vec<Step>, response_bytes: u32) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/bot123:synthetic/",
            listener.local_addr().unwrap()
        );
        let mut config = Config::default();
        config.limits.telegram_response_bytes = response_bytes;
        let mut client = TelegramClient::build(&config, endpoint, false).unwrap();
        client.retry_delays = Some(Arc::default());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let (seen, received) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            for step in steps {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let (path, payload) = loop {
                    let mut chunk = [0; 1_024];
                    let read = socket.read(&mut chunk).await.unwrap();
                    assert!(read > 0 && request.len() + read <= 16_384);
                    request.extend_from_slice(&chunk[..read]);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let header = std::str::from_utf8(&request[..end]).unwrap();
                        let length: usize = header
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        if request.len() == end + 4 + length {
                            break (
                                header
                                    .lines()
                                    .next()
                                    .unwrap()
                                    .split_whitespace()
                                    .nth(1)
                                    .unwrap()
                                    .to_owned(),
                                serde_json::from_slice(&request[end + 4..]).unwrap(),
                            );
                        }
                    }
                };
                recorded.lock().unwrap().push((path, payload));
                match step {
                    Step::Reply(bytes) => {
                        socket.write_all(&bytes).await.unwrap();
                    }
                    Step::Disconnect => (),
                    Step::Hang(prefix) => {
                        socket.write_all(&prefix).await.unwrap();
                        seen.send(()).await.unwrap();
                        std::future::pending::<()>().await;
                    }
                }
                seen.send(()).await.unwrap();
            }
        });
        Self {
            client,
            requests,
            seen: received,
            task,
        }
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[tokio::test]
async fn native_send_and_typing_use_typed_results() {
    let mock = Mock::new(
        vec![reply(200, SENT), reply(200, r#"{"ok":true,"result":true}"#)],
        8_192,
    )
    .await;
    let sent = mock
        .client
        .send_message(101, &formatted("<literal> 😀"))
        .await
        .unwrap();
    assert_eq!((sent.message_id, sent.date, sent.chat.id), (7, 100, 101));
    mock.client.clone().send_chat_action(101).await.unwrap();
    let requests = mock.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0],
        (
            "/bot123:synthetic/sendMessage".into(),
            json!({"chat_id":101,"text":"Session\n<literal> 😀", "entities":[{"type":"bold","offset":0,"length":7}]})
        )
    );
    assert_eq!(requests[1].1, json!({"chat_id":101,"action":"typing"}));
    assert!(!format!("{:?} {sent:?}", mock.client).contains("synthetic"));
}

#[tokio::test]
async fn chat_actions_return_rate_limits_without_internal_retries() {
    let mock = Mock::new(
        vec![reply(
            429,
            r#"{"ok":false,"error_code":429,"parameters":{"retry_after":30}}"#,
        )],
        8_192,
    )
    .await;
    assert_eq!(
        mock.client.send_chat_action(101).await.unwrap_err(),
        AppError::Telegram(TelegramError::Rejected {
            code: 429,
            retry_after_seconds: Some(30)
        })
    );
    assert_eq!(mock.count(), 1);
    assert!(
        mock.client
            .retry_delays
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn updates_preserve_reply_and_attachment_metadata_without_recursive_quotes() {
    let body = json!({"ok":true,"result":[{"update_id":12,"message":{
        "message_id":8,"date":101,"chat":{"id":101},"from":{"id":202,"is_bot":false},
        "reply_to_message":{"message_id":7,"text":"ignored quote","reply_to_message":{"message_id":6}},
        "caption":"private caption","document":{"file_id":"private-file","file_unique_id":"unique",
            "file_name":"report.txt","mime_type":"text/plain","file_size":42}
    }},{"update_id":13,"edited_message":{"text":"ignored"}}]});
    let mock = Mock::new(vec![reply(200, &body.to_string())], 8_192).await;
    let updates = mock.client.get_updates(Some(12), 2).await.unwrap();
    let message = updates[0].message.as_ref().unwrap();
    assert_eq!(message.reply_to_message.as_ref().unwrap().message_id, 7);
    assert_eq!(message.caption.as_deref(), Some("private caption"));
    assert_eq!(message.document.as_ref().unwrap().file_size, Some(42));
    assert!(updates[1].message.is_none());
    assert!(!format!("{updates:?}").contains("private"));
    assert_eq!(
        mock.requests.lock().unwrap()[0].1,
        json!({"offset":12,"limit":2,"timeout":45,"allowed_updates":["message"]})
    );
    assert!(mock.client.get_updates(Some(-1), 1).await.is_err());
    assert!(mock.client.get_updates(None, 0).await.is_err());
    assert_eq!(mock.count(), 1);
}

#[tokio::test]
async fn safe_retries_honor_server_delay_and_stop_at_the_attempt_budget() {
    let mock = Mock::new(
        vec![
            reply(
                429,
                r#"{"ok":false,"error_code":429,"parameters":{"retry_after":2}}"#,
            ),
            reply(502, "private proxy error"),
            reply(200, r#"{"ok":true,"result":[]}"#),
        ],
        1_024,
    )
    .await;
    assert!(mock.client.get_updates(None, 1).await.unwrap().is_empty());
    {
        let delays = mock.client.retry_delays.as_ref().unwrap().lock().unwrap();
        assert_eq!(delays[0], Duration::from_secs(2));
        assert!((Duration::from_millis(500)..Duration::from_secs(1)).contains(&delays[1]));
    }
    assert_eq!(mock.count(), 3);
    let mock = Mock::new((0..4).map(|_| reply(503, "unavailable")).collect(), 1_024).await;
    assert!(mock.client.get_webhook_info().await.is_err());
    assert_eq!(mock.count(), 3);
}

#[tokio::test]
async fn long_retry_after_and_webhook_conflicts_are_returned_without_mutation() {
    for (code, seconds) in [(429, Some(31)), (409, None), (401, None)] {
        let body = json!({"ok":false,"error_code":code,"parameters":{"retry_after":seconds},"description":"private token or message"}).to_string();
        let mock = Mock::new(vec![reply(code, &body), reply(200, SENT)], 1_024).await;
        let error = mock.client.get_updates(None, 1).await.unwrap_err();
        assert_eq!(
            error,
            AppError::Telegram(TelegramError::Rejected {
                code,
                retry_after_seconds: seconds
            })
        );
        assert!(!format!("{error:?} {error}").contains("private"));
        assert!(std::error::Error::source(&error).is_none());
        assert_eq!(mock.count(), 1);
        assert!(
            mock.client
                .retry_delays
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .is_empty()
        );
    }
    let mock = Mock::new(vec![reply(200, r#"{"ok":true,"result":{"url":"https://private.invalid/token","pending_update_count":3}}"#)], 1_024).await;
    let info = mock.client.get_webhook_info().await.unwrap();
    assert!(!info.url.is_empty());
    assert_eq!(info.pending_update_count, 3);
    assert!(!format!("{info:?}").contains("private"));
    assert!(
        mock.requests.lock().unwrap()[0]
            .0
            .ends_with("/getWebhookInfo")
    );
}

#[tokio::test]
async fn ambiguous_sends_are_never_repeated_or_redirected() {
    for step in [
        Step::Disconnect,
        reply(500, r#"{"ok":false,"error_code":500}"#),
        reply(200, "not json: private token"),
        reply(200, r#"{"ok":true,"result":{"message_id":7,"date":100,"chat":{"id":999}}}"#),
        reply(500, r#"{"ok":false,"error_code":400}"#),
        reply(200, &"x".repeat(1_025)),
        Step::Reply(b"HTTP/1.1 302 Redirect\r\nLocation: /private-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()),
    ] {
        let mock = Mock::new(vec![step, reply(200, SENT)], 1_024).await;
        assert_eq!(mock.client.send_message(101, &formatted("short")).await.unwrap_err(), TelegramError::DeliveryUncertain.into());
        assert_eq!(mock.count(), 1);
    }
    let mock = Mock::new(
        vec![reply(
            403,
            r#"{"ok":false,"error_code":403,"description":"private"}"#,
        )],
        1_024,
    )
    .await;
    assert_eq!(
        mock.client
            .send_message(101, &formatted("short"))
            .await
            .unwrap_err(),
        TelegramError::Rejected {
            code: 403,
            retry_after_seconds: None
        }
        .into()
    );
}

#[tokio::test]
async fn advertised_and_chunked_responses_are_bounded_before_decoding() {
    for step in [
        reply(200, &"x".repeat(129)),
        Step::Reply(format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n81\r\n{}\r\n0\r\n\r\n", "x".repeat(129)).into_bytes()),
    ] {
        let mock = Mock::new(vec![step], 128).await;
        assert_eq!(mock.client.get_webhook_info().await.unwrap_err(), TelegramError::ResponseTooLarge.into());
        assert_eq!(mock.count(), 1);
    }
    let mock = Mock::new(vec![reply(200, "{}")], 2).await;
    assert_eq!(
        mock.client.get_webhook_info().await.unwrap_err(),
        TelegramError::InvalidResponse.into()
    );
}

#[tokio::test]
async fn deadline_covers_a_response_that_stalls_after_headers() {
    let mut mock = Mock::new(
        vec![Step::Hang(
            b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{".to_vec(),
        )],
        1_024,
    )
    .await;
    let client = mock.client.clone();
    let mut call = tokio::spawn(async move { client.send_message(101, &formatted("short")).await });
    tokio::select! {
        seen = mock.seen.recv() => assert!(seen.is_some()),
        result = &mut call => panic!("call ended before the mock response: {result:?}"),
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    assert_eq!(
        call.await.unwrap().unwrap_err(),
        TelegramError::DeliveryUncertain.into()
    );
    assert_eq!(mock.count(), 1);
}

#[test]
fn retry_policy_has_finite_jitter_and_never_shortens_server_delays() {
    for attempt in 0..2 {
        for random in [0, 249, 500, u32::MAX] {
            let delay = retry_delay(&TelegramError::Transport, attempt, random).unwrap();
            let base = Duration::from_millis(250 << attempt);
            assert!(delay >= base && delay < base * 2);
        }
    }
    assert!(retry_delay(&TelegramError::Transport, 2, 0).is_none());
    assert!(retry_delay(&TelegramError::InvalidResponse, 0, 0).is_none());
    assert!(
        retry_delay(
            &TelegramError::Rejected {
                code: 429,
                retry_after_seconds: Some(u64::MAX)
            },
            0,
            0
        )
        .is_none()
    );
}
