use ratatui::style::Color;

use crate::data::oauth::OAuthCredential;
use crate::ui::theme::Theme;

pub(super) fn credential_status_message(cred: &OAuthCredential) -> String {
    if cred.access_token.is_none() {
        crate::ui::i18n::t("No OAuth token saved for this source yet.").to_string()
    } else if cred.is_expired() {
        crate::ui::i18n::t("Saved OAuth token is expired; waiting for rediscovery.").to_string()
    } else if let Some(error) = cred.stats.last_error.as_deref() {
        error.to_string()
    } else if cred.stats.attempt_count == 0 {
        crate::ui::i18n::t("Waiting for the first usage poll.").to_string()
    } else {
        crate::ui::i18n::t("Usage data is not available yet.").to_string()
    }
}

pub(super) fn credential_status_detail(cred: &OAuthCredential) -> Option<String> {
    let mut bits = Vec::new();
    if cred.stats.attempt_count > 0 {
        bits.push(format!(
            "{} {}",
            cred.stats.attempt_count,
            crate::ui::i18n::t("attempts"),
        ));
    }
    if cred.stats.call_count > 0 {
        bits.push(format!(
            "{} {}",
            cred.stats.call_count,
            crate::ui::i18n::t("ok"),
        ));
    }
    if cred.stats.rate_limit_count > 0 {
        bits.push(format!(
            "{} {}",
            cred.stats.rate_limit_count,
            crate::ui::i18n::t("rate-limited"),
        ));
    }
    if let Some(_last_fetch) = cred.stats.last_fetch {
        bits.push(format!(
            "{}{}",
            crate::ui::i18n::t("last success "),
            cred.stats.last_fetch_ago(),
        ));
    }

    if bits.is_empty() {
        None
    } else {
        Some(bits.join(" | "))
    }
}

pub(super) fn credential_status_color(cred: &OAuthCredential, t: &Theme) -> Color {
    if cred.is_codex() {
        t.text_dim
    } else if cred.access_token.is_none() || cred.is_expired() {
        t.warning
    } else if cred.stats.last_error.is_some() {
        t.error
    } else {
        t.text_dim
    }
}

pub(super) fn credential_poll_label(cred: &OAuthCredential) -> String {
    if cred.is_codex() {
        crate::ui::i18n::t("live from session").to_string()
    } else if cred.access_token.is_none() {
        crate::ui::i18n::t("no token").to_string()
    } else if cred.is_expired() {
        crate::ui::i18n::t("token expired").to_string()
    } else if cred.stats.call_count > 0 {
        format!(
            "{}{}",
            crate::ui::i18n::t("last success "),
            cred.stats.last_fetch_ago(),
        )
    } else if cred.stats.attempt_count > 0 {
        format!("{} attempt(s)", cred.stats.attempt_count)
    } else {
        crate::ui::i18n::t("awaiting first poll").to_string()
    }
}
