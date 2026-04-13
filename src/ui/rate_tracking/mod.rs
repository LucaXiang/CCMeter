mod credential_cards;
mod credential_status;
mod forecast;
mod gradient;
mod helpers;
mod kpi_bar;
mod live_summary;
mod rate_limits_table;
mod session_bars;
mod session_chart;
mod session_forecast;
mod usage_timeline;

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::data::index::EventIndex;
use crate::data::oauth::OAuthCredential;
use crate::data::rate_history::RateHistory;
use crate::data::rate_limits::RateLimitHit;

use super::theme::theme;

use credential_cards::{count_card_height, render_credential_cards};
use forecast::build_forecast_snapshot;
use kpi_bar::render_kpi_bar;
use live_summary::render_live_summary;
use rate_limits_table::render_rate_limits;
use session_bars::compute_session_bars;
use session_chart::render_session_chart;
use session_forecast::render_session_forecast;
use usage_timeline::render_usage_timeline;

#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    hits: &[RateLimitHit],
    source_names: &[String],
    source_roots: &[Option<String>],
    credentials: &[OAuthCredential],
    selected: Option<usize>,
    index: &EventIndex,
    rate_history: &RateHistory,
    reloading: bool,
    tick: usize,
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(44)])
        .split(outer[1]);

    // Filter rate limits by the selected credential's source_root
    let selected_root: Option<String> = selected
        .filter(|&i| i < credentials.len())
        .map(|i| credentials[i].source_root.to_string_lossy().to_string());

    let filtered_hits: Vec<&RateLimitHit> = match &selected_root {
        Some(root) => hits.iter().filter(|h| h.source_root == *root).collect(),
        None => hits.iter().collect(),
    };

    // Build credential roots once so rate-limits table and cards share the same color mapping
    let credential_roots: Vec<String> = credentials
        .iter()
        .map(|c| c.source_root.to_string_lossy().to_string())
        .collect();

    let selected_cred = selected
        .filter(|&i| i < credentials.len())
        .map(|i| &credentials[i]);
    let selected_minute_tokens = index.build_minute_tokens(selected_root.as_deref(), None);
    let forecast = build_forecast_snapshot(selected_cred, &selected_minute_tokens);

    render_live_summary(
        frame,
        outer[0],
        selected_cred,
        source_names,
        source_roots,
        forecast.as_ref(),
        tick,
    );

    render_rate_limits(
        frame,
        columns[0],
        &filtered_hits,
        source_names,
        source_roots,
        &credential_roots,
        selected_root.is_none(),
    );

    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(count_card_height(credentials)),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Min(4),
        ])
        .split(columns[1]);

    render_credential_cards(
        frame,
        right_rows[0],
        credentials,
        source_names,
        source_roots,
        selected,
        &credential_roots,
    );

    let bars = compute_session_bars(
        &filtered_hits,
        index,
        selected_root.as_deref(),
        selected_cred,
        rate_history,
    );
    render_kpi_bar(frame, right_rows[1], &bars, &filtered_hits);

    render_session_forecast(frame, right_rows[2], forecast.as_ref(), selected_cred, tick);
    render_usage_timeline(
        frame,
        right_rows[3],
        &selected_minute_tokens,
        selected_cred,
        tick,
    );

    render_session_chart(frame, right_rows[4], &bars, tick);

    // Footer
    render_footer(frame, outer[2], reloading);
}

fn render_footer(frame: &mut Frame, area: Rect, reloading: bool) {
    let t = theme();
    if reloading {
        let footer = Paragraph::new(Span::styled("⟳ Reloading…", Style::default().fg(t.warning)))
            .alignment(Alignment::Center);
        frame.render_widget(footer, area);
    } else {
        let text = "←→ Select source   r Refresh   t Dashboard   q Quit";
        let footer = Paragraph::new(Span::styled(text, Style::default().fg(t.text_dim)))
            .alignment(Alignment::Center);
        frame.render_widget(footer, area);
    }
}
