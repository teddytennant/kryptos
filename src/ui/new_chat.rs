//! "New chat" modal — wired to the `+` button in the sidebar header.
//!
//! Lets the user start a conversation with a Signal contact by phone
//! number (E.164). The new chat is upserted into the cache with a
//! current-time `last_message_ts` so it lands in the
//! `list_active_conversations` filter and shows up in the sidebar
//! immediately, even before the first message is sent.
//!
//! Telegram is intentionally out of scope for v1 — picking a Telegram
//! peer needs a username/picker (you can't just type a phone number),
//! and that flow will live next to this one once the contact picker
//! lands.

use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use adw::prelude::*;

use crate::cache::models::Conversation;
use crate::messenger::{Backend, ChatId};

use super::AsyncCtx;

/// Open the new-chat dialog. `on_started` fires once the conversation
/// has been written to the cache; the caller uses it to refresh the
/// sidebar and select the new row.
pub fn present_new_chat(
    parent: &impl IsA<gtk::Window>,
    ctx: Arc<AsyncCtx>,
    toast_overlay: adw::ToastOverlay,
    on_started: impl Fn(ChatId) + 'static,
) {
    let dlg = adw::AlertDialog::builder()
        .heading("New chat")
        .body("Enter a phone number in E.164 format, like +14155552671. The chat appears in the sidebar even if you haven't messaged this person yet.")
        .build();

    let entry = gtk::Entry::builder()
        .placeholder_text("+14155552671")
        .activates_default(true)
        .text("+")
        .build();
    dlg.set_extra_child(Some(&entry));

    dlg.add_response("cancel", "Cancel");
    dlg.add_response("start", "Start chat");
    dlg.set_response_appearance("start", adw::ResponseAppearance::Suggested);
    dlg.set_default_response(Some("start"));
    dlg.set_close_response("cancel");

    let on_started = Rc::new(on_started);
    let entry_for_handler = entry.clone();
    dlg.connect_response(
        None,
        move |dialog, response| {
            if response != "start" {
                dialog.close();
                return;
            }
            let raw = entry_for_handler.text().to_string();
            let trimmed = raw.trim();
            match parse_e164(trimmed) {
                Ok(number) => {
                    let chat_id = ChatId::new(Backend::Signal, number.clone());
                    let now = now_ms();
                    let res = ctx.runtime.block_on(ctx.cache.upsert_conversation(
                        &Conversation {
                            id: chat_id.to_wire(),
                            name: Some(number.clone()),
                            group_id: None,
                            last_message_ts: Some(now),
                            unread_count: 0,
                            archived: false,
                            muted_until: None,
                        },
                    ));
                    match res {
                        Ok(()) => {
                            on_started(chat_id);
                            dialog.close();
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "new-chat upsert failed");
                            toast(&toast_overlay, &format!("Couldn't start chat: {e}"));
                            // Leave dialog open so the user can retry.
                        }
                    }
                }
                Err(msg) => {
                    toast(&toast_overlay, msg);
                    // Leave dialog open so the user can fix the input.
                }
            }
        },
    );

    dlg.present(Some(parent.as_ref()));
}

fn parse_e164(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('+') {
        return Err("Number must start with + (E.164 format)");
    }
    let digits: String = trimmed.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 7 {
        return Err("That's too short for a phone number");
    }
    if digits.len() > 19 {
        return Err("That's too long for a phone number");
    }
    Ok(format!("+{digits}"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn toast(overlay: &adw::ToastOverlay, msg: &str) {
    overlay.add_toast(adw::Toast::builder().title(msg).timeout(4).build());
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_e164_canonical() {
        assert_eq!(parse_e164("+14155552671"), Ok("+14155552671".to_string()));
    }

    #[test]
    fn parse_e164_strips_formatting() {
        assert_eq!(
            parse_e164("+1 (415) 555-2671"),
            Ok("+14155552671".to_string())
        );
    }

    #[test]
    fn parse_e164_requires_plus() {
        assert!(parse_e164("14155552671").is_err());
    }

    #[test]
    fn parse_e164_rejects_short() {
        assert!(parse_e164("+12345").is_err());
    }
}
