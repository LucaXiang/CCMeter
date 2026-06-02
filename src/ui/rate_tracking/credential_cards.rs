use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::data::oauth::{OAuthCredential, UsageReport, UsageStats, UsageWindow};
use crate::ui::theme::theme;

use super::credential_status::{
    credential_status_color, credential_status_detail, credential_status_message,
};
use super::gradient::{gradient_bar_line, util_color};
use super::helpers::{credential_display_name, format_reset, source_color_index};

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
    selected: Option<usize>,
    credential_roots: &[String],
    week_costs: &[f64],
    month_costs: &[f64],
) {
    let t = theme();

    if credentials.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.border))
            .title(Span::styled(
                crate::ui::i18n::t(" No OAuth "),
                Style::default().fg(t.text_dim),
            ));
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
        let week_cost = week_costs.get(i).copied().unwrap_or(0.0);
        let month_cost = month_costs.get(i).copied().unwrap_or(0.0);
        render_card(
            frame,
            cols[i],
            cred,
            credential_roots,
            is_selected,
            week_cost,
            month_cost,
        );
    }
}

fn render_card(
    frame: &mut Frame,
    area: Rect,
    cred: &OAuthCredential,
    credential_roots: &[String],
    is_selected: bool,
    week_cost: f64,
    month_cost: f64,
) {
    let t = theme();
    let root_str = cred.source_root.to_string_lossy().to_string();
    let name = credential_display_name(&root_str);
    let color_idx = source_color_index(&root_str, credential_roots);
    let color = t.rainbow[color_idx % t.rainbow.len()];

    let sub = cred.subscription_type.as_deref().unwrap_or("?");
    let title_left = format!(" {} ({}) ", name, sub);

    // Title bar, right-aligned: trailing-7d API-equivalent spend (never an
    // empty $0.00/$300.00; Codex shows a $ too) plus a value multiplier vs the
    // subscription price (30d spend ÷ plan price), or a real overage charge.
    let title_right = spend_title(week_cost, month_cost, cred);

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

/// Right-side title spans: the real overage bar when the provider is actually
/// charging overage, otherwise the trailing-7d API-equivalent spend plus a
/// value multiplier (30d spend ÷ subscription price) when the plan is known.
fn spend_title(week_cost: f64, month_cost: f64, cred: &OAuthCredential) -> Vec<Span<'static>> {
    let usage = cred.usage.as_ref();
    let has_overage_charge = usage
        .and_then(|u| u.extra_usage.as_ref())
        .map(|e| e.is_enabled && e.used_credits.unwrap_or(0.0) > 0.0)
        .unwrap_or(false);
    if has_overage_charge {
        return extra_usage_title(usage);
    }
    let t = theme();
    let cost_str = crate::data::models::format_cost(week_cost);
    let week_label = format!("{}{}", cost_str, crate::ui::i18n::t(" ·7d "));
    let mut spans = vec![Span::styled(
        week_label,
        Style::default().fg(t.cost).add_modifier(Modifier::BOLD),
    )];
    if let Some(price) = cred
        .subscription_type
        .as_deref()
        .and_then(|plan| crate::data::models::plan_monthly_usd(cred.is_codex(), plan))
        && price > 0.0
        && month_cost > 0.0
    {
        let value_label = format!(
            "{}{:.0}{}",
            crate::ui::i18n::t("· "),
            month_cost / price,
            crate::ui::i18n::t("× value "),
        );
        spans.push(Span::styled(
            value_label,
            Style::default().fg(t.lines_positive),
        ));
    }
    spans
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
            "{}{} ({}req, {}err)",
            crate::ui::i18n::t("polled "),
            stats.last_fetch_ago(),
            stats.call_count,
            stats.rate_limit_count,
        ),
        Style::default().fg(t.text_dim),
    )]);
    frame.render_widget(Paragraph::new(status), rows[items.len()]);
}
