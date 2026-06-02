use chrono::TimeZone;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Row, Table};

use crate::data::models::{format_cost, per_model_cost_split};
use crate::data::rate_limits::RateLimitHit;
use crate::ui::theme::theme;

use super::helpers::{credential_display_name, source_color_index};

pub(super) fn render_rate_limits(
    frame: &mut Frame,
    area: Rect,
    hits: &[&RateLimitHit],
    credential_roots: &[String],
    show_source: bool,
) {
    let t = theme();

    // `hits` is already sorted descending by `merge_fresh_hits`.
    let rows: Vec<Row> = hits
        .iter()
        .map(|hit| {
            let local_ts = chrono::Local.from_utc_datetime(&hit.timestamp.naive_utc());
            let date_str = local_ts.format("%m-%d %H:%M").to_string();
            let duration_str = match hit.session_duration_min {
                Some(min) if min >= 1.0 => {
                    let h = min as u64 / 60;
                    let m = min as u64 % 60;
                    if h > 0 {
                        format!("{}h{:02}", h, m)
                    } else {
                        format!("{}m", m)
                    }
                }
                Some(_) => crate::ui::i18n::t("<1m").to_string(),
                None => "—".to_string(),
            };
            let (in_c, cache_c, out_c) = per_model_cost_split(&hit.per_model);
            let total_cost = in_c + cache_c + out_c;
            let cost_str = if total_cost > 0.0 {
                format_cost(total_cost)
            } else {
                "—".to_string()
            };

            if show_source {
                let name = credential_display_name(&hit.source_root);
                let color_idx = source_color_index(&hit.source_root, credential_roots);
                let color = t.rainbow[color_idx % t.rainbow.len()];
                Row::new(vec![
                    Cell::from(date_str).style(Style::default().fg(t.text_primary)),
                    Cell::from(name.to_string()).style(Style::default().fg(color)),
                    Cell::from(duration_str).style(Style::default().fg(t.text_secondary)),
                ])
            } else {
                Row::new(vec![
                    Cell::from(date_str).style(Style::default().fg(t.text_primary)),
                    Cell::from(duration_str).style(Style::default().fg(t.text_secondary)),
                    Cell::from(cost_str).style(Style::default().fg(t.tokens_in)),
                ])
            }
        })
        .collect();

    let (header, widths) = if show_source {
        (
            Row::new(vec![
                Cell::from(crate::ui::i18n::t("Date"))
                    .style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
                Cell::from(crate::ui::i18n::t("Source"))
                    .style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
                Cell::from(crate::ui::i18n::t("Dur"))
                    .style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
            ])
            .bottom_margin(1),
            vec![
                Constraint::Length(12),
                Constraint::Min(8),
                Constraint::Length(6),
            ],
        )
    } else {
        (
            Row::new(vec![
                Cell::from(crate::ui::i18n::t("Date"))
                    .style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
                Cell::from(crate::ui::i18n::t("Dur"))
                    .style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
                Cell::from(crate::ui::i18n::t("Cost"))
                    .style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
            ])
            .bottom_margin(1),
            vec![
                Constraint::Length(12),
                Constraint::Length(6),
                Constraint::Min(7),
            ],
        )
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(t.border))
                .title(Span::styled(
                    format!(
                        "{}{}{} ",
                        crate::ui::i18n::t(" Recent rate-limit hits ("),
                        hits.len(),
                        crate::ui::i18n::t(")"),
                    ),
                    Style::default()
                        .fg(t.heatmap_title)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .column_spacing(1);

    frame.render_widget(table, area);
}
