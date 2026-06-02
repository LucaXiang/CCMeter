use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::data::oauth::OAuthCredential;
use crate::ui::theme::theme;

use super::credential_status::{credential_status_color, credential_status_message};
use super::forecast::ForecastSnapshot;
use super::gradient::gradient_color;
use super::helpers::{blinking_dot_span, format_duration_label, format_rate_label};

pub(super) fn render_session_forecast(
    frame: &mut Frame,
    area: Rect,
    forecast: Option<&ForecastSnapshot>,
    selected_cred: Option<&OAuthCredential>,
    tick: usize,
) {
    let t = theme();
    let title = Line::from(vec![Span::styled(
        crate::ui::i18n::t(" Session pacing "),
        Style::default()
            .fg(t.heatmap_title)
            .add_modifier(Modifier::BOLD),
    )]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 20 {
        return;
    }

    let Some(snapshot) = forecast else {
        let status_color = selected_cred
            .map(|cred| credential_status_color(cred, t))
            .unwrap_or(t.text_dim);
        let msg = Paragraph::new(Span::styled(
            selected_cred
                .map(credential_status_message)
                .unwrap_or_else(|| {
                    crate::ui::i18n::t(
                        "Select a source to inspect its current session.",
                    )
                    .to_string()
                }),
            Style::default().fg(status_color),
        ));
        frame.render_widget(msg, inner);
        return;
    };

    let total_window_min = 300.0f64;
    let start_str = format!("{}", snapshot.session_start_local.format("%H:%M"));
    let end_str = format!("{}", snapshot.session_end_local.format("%H:%M"));
    let bar_width = inner
        .width
        .saturating_sub(start_str.len() as u16 + end_str.len() as u16 + 2)
        as usize;

    let now_pos = (snapshot.elapsed_min / total_window_min).clamp(0.0, 1.0);
    let filled = (now_pos * bar_width as f64) as usize;

    let mut bar_spans: Vec<Span> = Vec::new();
    bar_spans.push(Span::styled(
        format!("{} ", start_str),
        Style::default().fg(t.text_dim),
    ));
    for i in 0..bar_width {
        if i < filled {
            bar_spans.push(Span::styled(
                "█",
                Style::default().fg(gradient_color(snapshot.utilization_pct / 100.0)),
            ));
        } else {
            bar_spans.push(Span::styled("░", Style::default().fg(t.empty_bar)));
        }
    }
    bar_spans.push(Span::styled(
        format!(" {}", end_str),
        Style::default().fg(t.text_dim),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(bar_spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let marker_col = (start_str.len() + 1) + filled;
    let marker_col = marker_col.min(inner.width as usize - 4);
    let mut marker_spans: Vec<Span> = Vec::new();
    if marker_col > 0 {
        marker_spans.push(Span::raw(" ".repeat(marker_col)));
    }
    marker_spans.push(Span::styled(
        crate::ui::i18n::t("^now"),
        Style::default().fg(t.text_secondary),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(marker_spans)),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    let outlook = if let Some(mins) = snapshot.minutes_to_limit {
        if mins < snapshot.remaining_min {
            let eta = snapshot
                .limit_time_local
                .map(|ts| ts.format("%H:%M").to_string())
                .unwrap_or_else(|| crate::ui::i18n::t("soon").to_string());
            format!(
                "{}{} ({})",
                crate::ui::i18n::t("hit in "),
                format_duration_label(mins),
                eta,
            )
        } else {
            format!(
                "~{:.0}{}",
                snapshot.projected_pct_at_reset,
                crate::ui::i18n::t("% at reset"),
            )
        }
    } else {
        crate::ui::i18n::t("no forecast yet").to_string()
    };

    let status_line = Line::from(vec![
        blinking_dot_span(tick, snapshot.status.color(t)),
        Span::styled(
            snapshot.status.label(),
            Style::default()
                .fg(snapshot.status.color(t))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  |  ", Style::default().fg(t.text_dim)),
        Span::styled(outlook, Style::default().fg(t.text_primary)),
        Span::styled("  |  ", Style::default().fg(t.text_dim)),
        Span::styled(
            format!(
                "{}{} | {}{}",
                crate::ui::i18n::t("current "),
                format_rate_label(snapshot.rate_per_min),
                crate::ui::i18n::t("safe "),
                format_rate_label(snapshot.safe_rate_per_min),
            ),
            Style::default().fg(t.text_secondary),
        ),
    ]);

    frame.render_widget(
        Paragraph::new(status_line),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );
}
