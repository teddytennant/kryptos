//! Incoming D-Bus signal stream from signal-cli.
//!
//! Subscribes to per-account signals (currently `MessageReceived`) and
//! forwards them to consumers as a [`tokio::sync::mpsc::UnboundedReceiver`]
//! of [`Event`]. The forwarding task self-terminates when the receiver
//! is dropped.

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::core::Result;
use crate::dbus::client::SignalClient;

/// A normalized event derived from a signal-cli D-Bus signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    MessageReceived {
        ts: i64,
        sender: String,
        group_id: Option<Vec<u8>>,
        body: String,
        attachments: Vec<String>,
    },
    // Extend as the proxy gains more signals (read receipts, sync, etc.).
}

/// Subscribe to incoming D-Bus signals for `account`.
///
/// The returned receiver yields [`Event`]s. The background forwarder
/// drops itself the moment the receiver is dropped.
pub async fn subscribe(
    client: &SignalClient,
    account: &str,
) -> Result<mpsc::UnboundedReceiver<Event>> {
    let proxy = client.account(account).await?;
    let mut stream = proxy.receive_message_received().await?;

    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        while let Some(sig) = stream.next().await {
            let args = match sig.args() {
                Ok(a) => a,
                Err(e) => {
                    warn!(error = %e, "failed to decode MessageReceived signal");
                    continue;
                }
            };
            let event = Event::MessageReceived {
                ts: args.timestamp,
                sender: args.sender,
                group_id: normalize_group_id(args.group_id),
                body: args.message,
                attachments: args.attachments,
            };
            if tx.send(event).is_err() {
                debug!("MessageReceived consumer dropped; stopping forwarder");
                break;
            }
        }
    });

    Ok(rx)
}

/// Pure helper: signal-cli emits an empty `Vec<u8>` to mean "1:1 message,
/// not a group". Map that to `None` so consumers don't have to special-case.
pub(crate) fn normalize_group_id(id: Vec<u8>) -> Option<Vec<u8>> {
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_group_id_becomes_none() {
        assert_eq!(normalize_group_id(Vec::new()), None);
    }

    #[test]
    fn non_empty_group_id_is_some() {
        assert_eq!(normalize_group_id(vec![1, 2, 3]), Some(vec![1, 2, 3]));
    }
}
