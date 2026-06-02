use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::data::models::format_tokens;
use crate::data::oauth::OAuthCredential;
use crate::ui::theme;

use super::credential_status::{
    credential_poll_label, credential_status_color, credential_status_message,
};
use super::forecast::ForecastSnapshot;
use super::helpers::{
    blinking_dot_span, credential_display_name, format_duration_label, format_rate_label,
    truncate_text,
};

pub(super) fn render_live_summary(
    frame: &mut Frame,
    area: Rect,
    selected_cred: Option<&OAuthCredential>,
    forecast: Option<&ForecastSnapshot>,
    tick: usize,
) {
    let t = theme::theme();
    let border_color = forecast
        .map(|snapshot| snapshot.status.color(t))
        .unwrap_or(t.border_highlight);
    let (star, star_style) = theme::star_span(tick);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(star, star_style),
            Span::styled(
                crate::ui::i18n::t(" Live rate status "),
                Style::default()
                    .fg(t.heatmap_title)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 20 {
        return;
    }

    let Some(cred) = selected_cred else {
        frame.render_widget(
            Paragraph::new(crate::ui::i18n::t(
                "Use left/right to choose the OAuth source you want to monitor.",
            )),
            inner,
        );
        return;
    };

    let root = cred.source_root.to_string_lossy().to_string();
    let source = credential_display_name(&root);
    let sub = cred.subscription_type.as_deref().unwrap_or("?");
    let width = inner.width as usize;
    let source_label = truncate_text(&format!("{} ({})", source, sub), width.max(8));
    let polled_label = credential_poll_label(cred);

    let top = if width >= source_label.chars().count() + polled_label.chars().count() + 5 {
        Line::from(vec![
            Span::styled(
                source_label,
                Style::default()
                    .fg(t.tokens_out)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" | ", Style::default().fg(t.text_dim)),
            Span::styled(polled_label, Style::default().fg(t.text_secondary)),
        ])
    } else {
        Line::from(vec![Span::styled(
            source_label,
            Style::default()
                .fg(t.tokens_out)
                .add_modifier(Modifier::BOLD),
        )])
    };
    frame.render_widget(
        Paragraph::new(top),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let Some(snapshot) = forecast else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                credential_status_message(cred),
                Style::default().fg(credential_status_color(cred, t)),
            )),
            Rect::new(
                inner.x,
                inner.y + 1,
                inner.width,
                inner.height.saturating_sub(1),
            ),
        );
        return;
    };

    let status_color = snapshot.status.color(t);
    let middle_suffix = if width >= 60 {
        format!(
            " | 5h {:.0}% | {} {} | {} {}",
            snapshot.utilization_pct,
            crate::ui::i18n::t("reset"),
            snapshot.session_end_local.format("%H:%M"),
            crate::ui::i18n::t("elapsed"),
            format_duration_label(snapshot.elapsed_min),
        )
    } else if width >= 42 {
        format!(
            " | {:.0}% | {} {}",
            snapshot.utilization_pct,
            crate::ui::i18n::t("reset"),
            snapshot.session_end_local.format("%H:%M"),
        )
    } else {
        format!(" | {:.0}%", snapshot.utilization_pct)
    };
    let middle = Line::from(vec![
        blinking_dot_span(tick, status_color),
        Span::styled(
            snapshot.status.label(),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            truncate_text(
                &middle_suffix,
                width.saturating_sub(snapshot.status.label().len() + 2),
            ),
            Style::default().fg(t.text_secondary),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(middle),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    let outlook = if let Some(mins) = snapshot.minutes_to_limit {
        if mins < snapshot.remaining_min {
            let eta = snapshot
                .limit_time_local
                .map(|ts| ts.format("%H:%M").to_string())
                .unwrap_or_else(|| crate::ui::i18n::t("soon").to_string());
            format!(
                "{}{} {}{}{}{}",
                crate::ui::i18n::t("Hit in "),
                format_duration_label(mins),
                crate::ui::i18n::t("at current pace (around "),
                eta,
                crate::ui::i18n::t(")."),
                "",
            )
        } else {
            format!(
                "{}{:.0}{}",
                crate::ui::i18n::t("On track for ~"),
                snapshot.projected_pct_at_reset,
                crate::ui::i18n::t("% at reset."),
            )
        }
    } else {
        crate::ui::i18n::t("No hit forecast yet.").to_string()
    };
    let metrics = if width >= 70 {
        format!(
            "{}{} | {}{} | {}{}",
            crate::ui::i18n::t("Current "),
            format_rate_label(snapshot.rate_per_min),
            crate::ui::i18n::t("Safe "),
            format_rate_label(snapshot.safe_rate_per_min),
            crate::ui::i18n::t("Session "),
            format_tokens(snapshot.session_tokens),
        )
    } else if width >= 52 {
        format!(
            "{}{} | {}{}",
            crate::ui::i18n::t("Current "),
            format_rate_label(snapshot.rate_per_min),
            crate::ui::i18n::t("Safe "),
            format_rate_label(snapshot.safe_rate_per_min),
        )
    } else {
        format!(
            "{}{}",
            crate::ui::i18n::t("Current "),
            format_rate_label(snapshot.rate_per_min),
        )
    };
    let bottom = if width >= 58 {
        Line::from(vec![
            Span::styled(
                truncate_text(&outlook, width.saturating_sub(metrics.chars().count() + 3)),
                Style::default().fg(t.text_primary),
            ),
            Span::styled(" | ", Style::default().fg(t.text_dim)),
            Span::styled(metrics, Style::default().fg(t.text_secondary)),
        ])
    } else {
        Line::from(vec![Span::styled(
            truncate_text(&outlook, width.max(8)),
            Style::default().fg(t.text_primary),
        )])
    };
    frame.render_widget(
        Paragraph::new(bottom),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );
}
