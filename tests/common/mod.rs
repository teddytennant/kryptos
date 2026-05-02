//! Shared helpers for integration tests under `tests/`.
//!
//! The hub-internal `MockBackend` in `src/messenger/hub.rs` covers
//! white-box behaviour against private items; this module is the
//! black-box equivalent every `tests/*.rs` file can pull in.
//!
//! `tests/common.rs` is *not* a test file itself — Cargo treats every
//! `tests/*.rs` as a separate integration binary, but a `mod common;`
//! declaration in another integration test file pulls this in as a
//! plain module. We mark items `#[allow(dead_code)]` so a per-file
//! integration test doesn't have to use every helper.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use kryptos::core::Result;
use kryptos::messenger::{
    hub::MessengerHub, Backend, ChatId, ConversationSummary, Event, MessengerBackend,
    NormalizedMessage,
};
use tokio::sync::{broadcast, mpsc};

/// Black-box mock used by integration tests. Mirrors the hub-internal
/// mock but lives outside the crate so it can only touch the public
/// API — i.e. it proves the public surface is sufficient.
pub struct MockBackend {
    kind: Backend,
    bus: broadcast::Sender<Event>,
    /// Every (recipient, body) pair that flowed through `send_message`,
    /// in call order.
    sent: StdMutex<Vec<(ChatId, String)>>,
    next_ts: StdMutex<i64>,
}

impl MockBackend {
    pub fn new(kind: Backend) -> Arc<Self> {
        let (bus, _) = broadcast::channel(16);
        Arc::new(Self {
            kind,
            bus,
            sent: StdMutex::new(Vec::new()),
            next_ts: StdMutex::new(1_000),
        })
    }

    pub fn sent(&self) -> Vec<(ChatId, String)> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait]
impl MessengerBackend for MockBackend {
    fn backend(&self) -> Backend {
        self.kind
    }

    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>> {
        Ok(vec![ConversationSummary {
            id: ChatId::new(self.kind, "mock"),
            title: format!("{} mock", self.kind),
            last_message_ts: None,
            unread: 0,
        }])
    }

    async fn fetch_history(
        &self,
        _id: &ChatId,
        _limit: u32,
        _before_ts: Option<i64>,
    ) -> Result<Vec<NormalizedMessage>> {
        Ok(Vec::new())
    }

    async fn send_message(&self, id: &ChatId, body: &str, _attachments: &[PathBuf]) -> Result<i64> {
        self.sent
            .lock()
            .unwrap()
            .push((id.clone(), body.to_string()));
        let mut ts = self.next_ts.lock().unwrap();
        *ts += 1;
        Ok(*ts)
    }

    async fn mark_read(&self, _id: &ChatId) -> Result<()> {
        Ok(())
    }

    async fn typing(&self, _id: &ChatId, _on: bool) -> Result<()> {
        Ok(())
    }

    async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<Event>> {
        let mut bcast_rx = self.bus.subscribe();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(ev) = bcast_rx.recv().await {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

/// Build a hub pre-loaded with a Signal + Telegram mock and return it
/// alongside the mock handles so tests can both drive the hub and
/// inspect what each mock saw.
pub fn hub_with_signal_and_telegram() -> (MessengerHub, Arc<MockBackend>, Arc<MockBackend>) {
    let s = MockBackend::new(Backend::Signal);
    let t = MockBackend::new(Backend::Telegram);
    let mut hub = MessengerHub::new();
    hub.add(s.clone());
    hub.add(t.clone());
    (hub, s, t)
}
