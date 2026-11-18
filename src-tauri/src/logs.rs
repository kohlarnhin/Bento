use serde::Serialize;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep_until, Instant};

const MAX_BATCH: usize = 500;
const BATCH_DELAY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub text: String,
    pub stream: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub project_id: String,
    pub entries: Vec<Entry>,
}

pub fn is_error(line: &str) -> bool {
    let mut clean = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            clean.push(ch);
        } else if chars.next_if_eq(&'[').is_some() {
            for byte in chars.by_ref() {
                if ('@'..='~').contains(&byte) { break; }
            }
        } else if chars.next_if_eq(&']').is_some() {
            while let Some(byte) = chars.next() {
                if byte == '\u{7}' || (byte == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) { break; }
            }
        }
    }
    let words = clean.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_');
    if words.clone().any(|word| {
        let word = word.to_ascii_lowercase();
        matches!(word.as_str(), "traceback" | "failed") || word.ends_with("error") || word.ends_with("exception")
    }) {
        return true;
    }
    let http = words.clone().any(|word| matches!(word.to_ascii_uppercase().as_str(),
        "HTTP" | "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "STATUS"));
    http && clean.split_whitespace().any(|part| {
        let code = part.trim_matches(|ch| matches!(ch, '"' | '\'' | '(' | ')' | ',' | ':' | ']'));
        code.len() == 3 && code.parse::<u16>().is_ok_and(|code| (500..600).contains(&code))
    })
}

fn flush(batch: &mut Vec<Entry>, emit: &mut impl FnMut(Vec<Entry>)) {
    if !batch.is_empty() {
        emit(std::mem::take(batch));
    }
}

/// One receiver preserves stdout/stderr arrival order. The bounded channel also
/// applies backpressure without retaining an unbounded build's output in memory.
pub async fn aggregate(
    mut receiver: mpsc::Receiver<Entry>,
    mut finish: oneshot::Receiver<()>,
    mut emit: impl FnMut(Vec<Entry>),
) {
    let mut batch = Vec::new();
    let mut deadline = Instant::now() + BATCH_DELAY;
    loop {
        tokio::select! {
            biased;
            _ = &mut finish => {
                // A descendant may keep the pipes open after the root exits.
                // Close senders, drain queued output and finish before the exit log.
                receiver.close();
                while let Some(entry) = receiver.recv().await {
                    batch.push(entry);
                    if batch.len() == MAX_BATCH {
                        flush(&mut batch, &mut emit);
                    }
                }
                flush(&mut batch, &mut emit);
                return;
            }
            _ = sleep_until(deadline), if !batch.is_empty() => {
                flush(&mut batch, &mut emit);
            }
            entry = receiver.recv() => {
                let Some(entry) = entry else {
                    flush(&mut batch, &mut emit);
                    return;
                };
                if batch.is_empty() {
                    deadline = Instant::now() + BATCH_DELAY;
                }
                batch.push(entry);
                if batch.len() == MAX_BATCH {
                    flush(&mut batch, &mut emit);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(number: usize) -> Entry {
        Entry {
            text: number.to_string(),
            stream: if number % 2 == 0 { "stdout" } else { "stderr" }.into(),
        }
    }

    #[test]
    fn identifies_colored_errors_and_server_responses_once() {
        for line in ["\x1b[31mERROR\x1b[0m", "OperationalError: deadlock", "Traceback (most recent call last)", "request failed", "[API] GET /orders 503 · 10ms"] {
            assert!(is_error(line), "{line}");
        }
        for line in ["port 5000", "finished in 503 ms", "[WEB] GET /500 200", "[APP] WARNING changed"] {
            assert!(!is_error(line), "{line}");
        }
    }

    #[tokio::test]
    async fn batches_at_limit_and_flushes_close_in_arrival_order() {
        let (sender, receiver) = mpsc::channel(2048);
        let (_finish, finished) = oneshot::channel();
        for number in 0..1203 {
            sender.send(entry(number)).await.unwrap();
        }
        drop(sender);
        let mut batches = Vec::new();
        aggregate(receiver, finished, |batch| batches.push(batch)).await;
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            [500, 500, 203]
        );
        assert_eq!(
            batches.into_iter().flatten().collect::<Vec<_>>(),
            (0..1203).map(entry).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn flushes_partial_batch_while_channel_stays_open() {
        let (sender, receiver) = mpsc::channel(10);
        let (_finish, finished) = oneshot::channel();
        let (batches, mut output) = mpsc::unbounded_channel();
        let task = tokio::spawn(aggregate(receiver, finished, move |batch| {
            batches.send(batch).unwrap();
        }));
        sender.send(entry(0)).await.unwrap();
        let batch = tokio::time::timeout(Duration::from_millis(500), output.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch, [entry(0)]);
        sender.send(entry(1)).await.unwrap();
        drop(sender);
        task.await.unwrap();
        assert_eq!(output.recv().await.unwrap(), [entry(1)]);
        assert!(output.recv().await.is_none());
    }

    #[tokio::test]
    async fn bounded_finish_drains_pending_lines_and_closes_readers() {
        let (sender, receiver) = mpsc::channel(10);
        let (finish, finished) = oneshot::channel();
        sender.send(entry(0)).await.unwrap();
        sender.send(entry(1)).await.unwrap();
        finish.send(()).unwrap();
        let mut output = Vec::new();
        aggregate(receiver, finished, |batch| output.extend(batch)).await;
        assert_eq!(output, [entry(0), entry(1)]);
        assert!(sender.send(entry(2)).await.is_err());
    }
}
