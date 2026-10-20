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
