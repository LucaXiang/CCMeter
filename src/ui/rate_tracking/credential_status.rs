use ratatui::style::Color;

use crate::data::oauth::OAuthCredential;
use crate::ui::theme::Theme;

pub(super) fn credential_status_message(cred: &OAuthCredential) -> String {
    if cred.access_token.is_none() {
        "No OAuth token saved for this source yet.".to_string()
    } else if cred.is_expired() {
        "Saved OAuth token is expired; waiting for rediscovery.".to_string()
    } else if let Some(error) = cred.stats.last_error.as_deref() {
        error.to_string()
    } else if cred.stats.attempt_count == 0 {
        "Waiting for the first usage poll.".to_string()
    } else {
        "Usage data is not available yet.".to_string()
    }
}

pub(super) fn credential_status_detail(cred: &OAuthCredential) -> Option<String> {
    let mut bits = Vec::new();
    if cred.stats.attempt_count > 0 {
        bits.push(format!("{} attempts", cred.stats.attempt_count));
    }
    if cred.stats.call_count > 0 {
        bits.push(format!("{} ok", cred.stats.call_count));
    }
    if cred.stats.rate_limit_count > 0 {
        bits.push(format!("{} rate-limited", cred.stats.rate_limit_count));
    }
    if let Some(_last_fetch) = cred.stats.last_fetch {
        bits.push(format!("last success {}", cred.stats.last_fetch_ago()));
    }

    if bits.is_empty() {
        None
    } else {
        Some(bits.join(" | "))
    }
}

pub(super) fn credential_status_color(cred: &OAuthCredential, t: &Theme) -> Color {
    if cred.access_token.is_none() || cred.is_expired() {
        t.warning
    } else if cred.stats.last_error.is_some() {
        t.error
    } else {
        t.text_dim
    }
}

pub(super) fn credential_poll_label(cred: &OAuthCredential) -> String {
    if cred.access_token.is_none() {
        "no token".to_string()
    } else if cred.is_expired() {
        "token expired".to_string()
    } else if cred.stats.call_count > 0 {
        format!("last success {}", cred.stats.last_fetch_ago())
    } else if cred.stats.attempt_count > 0 {
        format!("{} attempt(s)", cred.stats.attempt_count)
    } else {
        "awaiting first poll".to_string()
    }
}
