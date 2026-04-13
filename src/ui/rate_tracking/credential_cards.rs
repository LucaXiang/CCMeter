use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::data::oauth::{OAuthCredential, UsageReport, UsageStats, UsageWindow};
use crate::ui::theme::theme;

use super::credential_status::{
    credential_status_color, credential_status_detail, credential_status_message,
};
use super::gradient::{gradient_bar_line, util_color};
use super::helpers::{format_reset, source_color_index, source_display_name};

pub(super) fn count_card_height(credentials: &[OAuthCredential]) -> u16 {
    if credentials.is_empty() {
        return 4;
    }
    let max_gauges = credentials
        .iter()
        .map(|c| count_gauges(c.usage.as_ref()))
        .max()
        .unwrap_or(0);
    (3 + max_gauges).max(4) as u16
}

fn count_gauges(usage: Option<&UsageReport>) -> usize {
    let Some(u) = usage else { return 0 };
    [
        u.five_hour.is_some(),
        u.seven_day.is_some(),
        u.seven_day_opus.is_some(),
        u.seven_day_sonnet.is_some(),
        u.seven_day_cowork.is_some(),
    ]
    .iter()
    .filter(|&&v| v)
    .count()
}

pub(super) fn render_credential_cards(
    frame: &mut Frame,
    area: Rect,
    credentials: &[OAuthCredential],
    source_names: &[String],
    source_roots: &[Option<String>],
    selected: Option<usize>,
    credential_roots: &[String],
) {
    let t = theme();

    if credentials.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.border))
            .title(Span::styled(" No OAuth ", Style::default().fg(t.text_dim)));
        frame.render_widget(block, area);
        return;
    }

    let max_gauges = credentials
        .iter()
        .map(|c| count_gauges(c.usage.as_ref()))
        .max()
        .unwrap_or(0);
    // border(2) + gauges + status(1)
    let card_h = (3 + max_gauges).max(4) as u16;

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(card_h), Constraint::Min(0)])
        .split(area);

    let constraints: Vec<Constraint> = credentials
        .iter()
        .map(|_| Constraint::Ratio(1, credentials.len() as u32))
        .collect();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(rows[0]);

    for (i, cred) in credentials.iter().enumerate() {
        let is_selected = selected == Some(i);
        render_card(
            frame,
            cols[i],
            cred,
            source_names,
            source_roots,
            credential_roots,
            is_selected,
        );
    }
}

fn render_card(
    frame: &mut Frame,
    area: Rect,
    cred: &OAuthCredential,
    source_names: &[String],
    source_roots: &[Option<String>],
    credential_roots: &[String],
    is_selected: bool,
) {
    let t = theme();
    let root_str = cred.source_root.to_string_lossy().to_string();
    let name = source_display_name(&root_str, source_names, source_roots);
    let color_idx = source_color_index(&root_str, credential_roots);
    let color = t.rainbow[color_idx % t.rainbow.len()];

    let sub = cred.subscription_type.as_deref().unwrap_or("?");
    let title_left = format!(" {} ({}) ", name, sub);

    // Extra usage in title bar (right-aligned)
    let title_right = extra_usage_title(cred.usage.as_ref());

    let border_color = if is_selected {
        t.border_highlight
    } else {
        t.border
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title(Span::styled(
            title_left,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));

    if !title_right.is_empty() {
        block = block.title_top(Line::from(title_right).alignment(Alignment::Right));
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &cred.usage {
        Some(usage) => render_compact_usage(frame, inner, usage, &cred.stats),
        None => {
            let mut lines = vec![Line::from(Span::styled(
                credential_status_message(cred),
                Style::default().fg(credential_status_color(cred, t)),
            ))];
            if let Some(detail) = credential_status_detail(cred) {
                lines.push(Line::from(Span::styled(
                    detail,
                    Style::default().fg(t.text_dim),
                )));
            }
            frame.render_widget(Paragraph::new(lines), inner);
        }
    }
}

/// Build extra usage spans for the block title (right side).
fn extra_usage_title(usage: Option<&UsageReport>) -> Vec<Span<'static>> {
    let Some(u) = usage else { return vec![] };
    let Some(extra) = &u.extra_usage else {
        return vec![];
    };
    if !extra.is_enabled || extra.used_credits.is_none() {
        return vec![];
    }

    let used = extra.used_credits.unwrap_or(0.0) / 100.0;
    let limit = extra.monthly_limit.unwrap_or(0.0) / 100.0;
    let util = extra.utilization.unwrap_or(0.0);

    vec![Span::styled(
        format!("${:.2}/${:.2} ({:.0}%) ", used, limit, util),
        Style::default()
            .fg(util_color(util))
            .add_modifier(Modifier::BOLD),
    )]
}

fn render_compact_usage(frame: &mut Frame, area: Rect, usage: &UsageReport, stats: &UsageStats) {
    let t = theme();

    let windows: &[(&str, Option<&UsageWindow>)] = &[
        ("5h", usage.five_hour.as_ref()),
        ("7d", usage.seven_day.as_ref()),
        ("opus", usage.seven_day_opus.as_ref()),
        ("sonnet", usage.seven_day_sonnet.as_ref()),
        ("cowork", usage.seven_day_cowork.as_ref()),
    ];
    let items: Vec<(&str, f64, String)> = windows
        .iter()
        .filter_map(|(label, w)| {
            w.map(|w| {
                (
                    *label,
                    w.utilization,
                    w.resets_at.as_deref().map(format_reset).unwrap_or_default(),
                )
            })
        })
        .collect();

    let mut constraints: Vec<Constraint> = Vec::new();
    for _ in &items {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // status line
    constraints.push(Constraint::Min(0));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (i, (label, pct, reset)) in items.iter().enumerate() {
        let ratio = (*pct / 100.0).clamp(0.0, 1.0);
        let line = gradient_bar_line(area.width, ratio, label, *pct, reset);
        frame.render_widget(Paragraph::new(line), rows[i]);
    }

    // Status line
    let status = Line::from(vec![Span::styled(
        format!(
            "polled {} ({}req, {}err)",
            stats.last_fetch_ago(),
            stats.call_count,
            stats.rate_limit_count,
        ),
        Style::default().fg(t.text_dim),
    )]);
    frame.render_widget(Paragraph::new(status), rows[items.len()]);
}
