//! Multi-backend orchestrator.
//!
//! The hub owns one or more [`MessengerBackend`] handles and presents
//! the rest of the app a single API: subscribe once, receive every
//! backend's events on the same channel; send to a [`ChatId`] without
//! caring which backend it belongs to.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::core::{Error, Result};
use crate::messenger::{
    Backend, ChatId, ConversationSummary, Event, MessengerBackend,
};

#[derive(Default)]
pub struct MessengerHub {
    backends: Vec<Arc<dyn MessengerBackend>>,
}

impl MessengerHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, backend: Arc<dyn MessengerBackend>) {
        self.backends.push(backend);
    }

    pub fn backends(&self) -> &[Arc<dyn MessengerBackend>] {
        &self.backends
    }

    /// Merge every backend's event stream into a single receiver.
    ///
    /// Each backend gets its own forwarder task. The merged receiver
    /// stays open as long as at least one backend is still emitting;
    /// dropping the receiver tears every forwarder down.
    pub async fn subscribe_all(&self) -> Result<mpsc::UnboundedReceiver<Event>> {
        let (tx, rx) = mpsc::unbounded_channel();
        for backend in &self.backends {
            let mut sub = match backend.subscribe().await {
                Ok(rx) => rx,
                Err(e) => {
                    warn!(backend = %backend.backend(), error = %e, "subscribe failed; skipping");
                    continue;
                }
            };
            let tx = tx.clone();
            let label = backend.backend();
            tokio::spawn(async move {
                while let Some(ev) = sub.recv().await {
                    if tx.send(ev).is_err() {
                        debug!(%label, "hub merger: consumer dropped");
                        break;
                    }
                }
            });
        }
        Ok(rx)
    }

    /// Route a send by [`ChatId::backend`].
    pub async fn send(
        &self,
        id: &ChatId,
        body: &str,
        attachments: &[PathBuf],
    ) -> Result<i64> {
        self.find(id.backend)
            .ok_or_else(|| Error::Config(format!("no backend registered for {}", id.backend)))?
            .send_message(id, body, attachments)
            .await
    }

    /// Concatenated conversation list across every backend. Per-backend
    /// failures are logged and skipped rather than failing the whole
    /// call so a flaky Telegram doesn't blank out the Signal sidebar.
    pub async fn list_all_conversations(&self) -> Vec<ConversationSummary> {
        let mut out = Vec::new();
        for backend in &self.backends {
            match backend.list_conversations().await {
                Ok(mut convos) => out.append(&mut convos),
                Err(e) => warn!(
                    backend = %backend.backend(),
                    error = %e,
                    "list_conversations failed; skipping"
                ),
            }
        }
        out
    }

    fn find(&self, kind: Backend) -> Option<&Arc<dyn MessengerBackend>> {
        self.backends.iter().find(|b| b.backend() == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messenger::{BackendExtras, NormalizedMessage};
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::broadcast;

    struct MockBackend {
        kind: Backend,
        bus: broadcast::Sender<Event>,
        sent: StdMutex<Vec<(ChatId, String)>>,
        next_ts: StdMutex<i64>,
    }

    impl MockBackend {
        fn new(kind: Backend) -> Arc<Self> {
            let (bus, _) = broadcast::channel(16);
            Arc::new(Self {
                kind,
                bus,
                sent: StdMutex::new(Vec::new()),
                next_ts: StdMutex::new(1_000),
            })
        }

        fn emit(&self, ev: Event) {
            // Send may transiently fail with "no subscribers"; that's
            // not interesting for the test, we always have a hub
            // subscriber wired by the time we emit.
            let _ = self.bus.send(ev);
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

        async fn send_message(
            &self,
            id: &ChatId,
            body: &str,
            _attachments: &[PathBuf],
        ) -> Result<i64> {
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

    fn msg(backend: Backend, native: &str, body: &str) -> NormalizedMessage {
        let extras = match backend {
            Backend::Signal => BackendExtras::Signal { group_id: None },
            Backend::Telegram => BackendExtras::Telegram { reply_to_msg_id: None },
        };
        NormalizedMessage {
            id: ChatId::new(backend, native),
            ts_ms: 1,
            sender: "tester".into(),
            body: Some(body.into()),
            attachments: Vec::new(),
            backend_extras: extras,
        }
    }

    #[tokio::test]
    async fn fan_in_merges_two_backends() {
        let a = MockBackend::new(Backend::Signal);
        let b = MockBackend::new(Backend::Telegram);
        let mut hub = MessengerHub::new();
        hub.add(a.clone());
        hub.add(b.clone());

        let mut rx = hub.subscribe_all().await.unwrap();

        // Give the per-backend forwarder tasks a tick to actually
        // subscribe to their broadcast bus before we publish.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        a.emit(Event::MessageReceived(msg(Backend::Signal, "+1", "from-signal")));
        b.emit(Event::MessageReceived(msg(Backend::Telegram, "9", "from-tg")));

        let mut got = Vec::new();
        for _ in 0..2 {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("hub recv timed out")
                .expect("hub stream closed");
            got.push(ev);
        }
        let bodies: Vec<_> = got
            .into_iter()
            .map(|e| match e {
                Event::MessageReceived(m) => (m.id.backend, m.body.unwrap()),
                other => panic!("unexpected event {other:?}"),
            })
            .collect();
        assert!(bodies.contains(&(Backend::Signal, "from-signal".into())));
        assert!(bodies.contains(&(Backend::Telegram, "from-tg".into())));
    }

    #[tokio::test]
    async fn send_routes_to_correct_backend() {
        let s = MockBackend::new(Backend::Signal);
        let t = MockBackend::new(Backend::Telegram);
        let mut hub = MessengerHub::new();
        hub.add(s.clone());
        hub.add(t.clone());

        hub.send(&ChatId::new(Backend::Signal, "+1"), "hi-s", &[])
            .await
            .unwrap();
        hub.send(&ChatId::new(Backend::Telegram, "9"), "hi-t", &[])
            .await
            .unwrap();

        let s_sent = s.sent.lock().unwrap().clone();
        let t_sent = t.sent.lock().unwrap().clone();
        assert_eq!(s_sent.len(), 1);
        assert_eq!(s_sent[0].1, "hi-s");
        assert_eq!(t_sent.len(), 1);
        assert_eq!(t_sent[0].1, "hi-t");
    }

    #[tokio::test]
    async fn send_to_unregistered_backend_errors() {
        let hub = MessengerHub::new();
        let err = hub
            .send(&ChatId::new(Backend::Signal, "+1"), "hi", &[])
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("no backend"));
    }

    #[tokio::test]
    async fn list_all_concatenates_per_backend() {
        let s = MockBackend::new(Backend::Signal);
        let t = MockBackend::new(Backend::Telegram);
        let mut hub = MessengerHub::new();
        hub.add(s);
        hub.add(t);
        let convos = hub.list_all_conversations().await;
        assert_eq!(convos.len(), 2);
        let backends: Vec<_> = convos.iter().map(|c| c.id.backend).collect();
        assert!(backends.contains(&Backend::Signal));
        assert!(backends.contains(&Backend::Telegram));
    }
}
