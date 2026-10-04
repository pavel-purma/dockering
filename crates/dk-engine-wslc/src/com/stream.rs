//! Long-lived blocking streams on dedicated MTA threads (spec 20 §5.4): handle readers (logs,
//! exec TTY) and the events `GetNext` loop live in the engine; this module provides the
//! reusable pieces: a cancellable handle reader feeding a `futures` mpsc, and a drop guard
//! that signals cancellation when the consumer drops the stream.

#![cfg(windows)]

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use dk_core::{EngineError, EngineResult};
use futures::Stream;
use futures::channel::mpsc;

use super::win32::{Event, OwnedHandle, ReadOutcome, read_cancellable};

/// Shared cancel signal: a manual-reset Win32 event (usable as the `GetNext` `CancelEvent` and
/// as an extra wait handle for overlapped reads).
#[derive(Debug, Clone)]
pub struct Cancel(Arc<Event>);

impl Cancel {
    pub fn new() -> EngineResult<Self> {
        Event::new()
            .map(|e| Self(Arc::new(e)))
            .map_err(|e| EngineError::protocol(format!("CreateEvent failed: {e}")))
    }
    pub fn fire(&self) {
        self.0.set();
    }
    pub fn is_fired(&self) -> bool {
        self.0.is_set()
    }
    pub fn event(&self) -> &Event {
        &self.0
    }
}

/// A `Stream` over an mpsc receiver that fires `cancel` when dropped, so the producer thread
/// unblocks (`SetEvent` → `GetNext` returns `E_ABORT`, overlapped reads are `CancelIoEx`ed).
pub struct CancelOnDrop<T> {
    rx: mpsc::Receiver<EngineResult<T>>,
    cancel: Cancel,
}

impl<T> CancelOnDrop<T> {
    pub fn new(rx: mpsc::Receiver<EngineResult<T>>, cancel: Cancel) -> Self {
        Self { rx, cancel }
    }
}

impl<T> Stream for CancelOnDrop<T> {
    type Item = EngineResult<T>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl<T> Drop for CancelOnDrop<T> {
    fn drop(&mut self) {
        self.cancel.fire();
    }
}

/// Blocking send from a producer thread; `false` when the consumer is gone (stop producing).
pub fn send_blocking<T>(tx: &mut mpsc::Sender<EngineResult<T>>, item: EngineResult<T>) -> bool {
    futures::executor::block_on(futures::SinkExt::send(tx, item)).is_ok()
}

/// Reads `h` until EOF / cancel / error, calling `on_data` per read. Returns `Ok(true)` on EOF,
/// `Ok(false)` on cancel. Runs on the calling (dedicated) thread.
pub fn pump_handle(
    h: &OwnedHandle,
    cancel: &Cancel,
    mut on_data: impl FnMut(Bytes) -> bool,
) -> EngineResult<bool> {
    let mut buf = BytesMut::zeroed(64 * 1024);
    loop {
        match read_cancellable(h, &mut buf, cancel.event()) {
            Ok(ReadOutcome::Data(n)) => {
                if !on_data(Bytes::copy_from_slice(&buf[..n])) {
                    return Ok(false);
                }
            }
            Ok(ReadOutcome::Eof) => return Ok(true),
            Ok(ReadOutcome::Cancelled) => return Ok(false),
            Err(e) => {
                super::dispatch::record(e.code().0);
                return Err(EngineError::protocol(format!(
                    "read failed: 0x{:08X} {}",
                    e.code().0 as u32,
                    e.message()
                )));
            }
        }
    }
}

/// Splits a byte stream into lines (keeping `\n`), carrying partial lines between reads.
#[derive(Debug, Default)]
pub struct LineSplitter {
    partial: BytesMut,
}

impl LineSplitter {
    /// Complete lines contained in `partial + data`.
    pub fn push(&mut self, data: &[u8]) -> Vec<Bytes> {
        self.partial.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(i) = self.partial.iter().position(|b| *b == b'\n') {
            out.push(self.partial.split_to(i + 1).freeze());
        }
        // Guard against unbounded partial lines (no newline for 1 MiB): flush as-is.
        if self.partial.len() > 1024 * 1024 {
            out.push(self.partial.split().freeze());
        }
        out
    }
    /// Remaining partial line at EOF.
    pub fn finish(&mut self) -> Option<Bytes> {
        (!self.partial.is_empty()).then(|| self.partial.split().freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitter_keeps_partials() {
        let mut s = LineSplitter::default();
        assert_eq!(s.push(b"a\nb"), vec![Bytes::from_static(b"a\n")]);
        assert_eq!(
            s.push(b"c\n\nd"),
            vec![Bytes::from_static(b"bc\n"), Bytes::from_static(b"\n")]
        );
        assert_eq!(s.finish(), Some(Bytes::from_static(b"d")));
        assert_eq!(s.finish(), None);
    }

    #[test]
    fn cancel_on_drop_fires() {
        let cancel = Cancel::new().expect("event");
        let (_tx, rx) = mpsc::channel::<EngineResult<u8>>(1);
        let s = CancelOnDrop::new(rx, cancel.clone());
        assert!(!cancel.is_fired());
        drop(s);
        assert!(cancel.is_fired());
    }
}
