use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::data::models::format_cost;
use crate::data::rate_limits::RateLimitHit;
use crate::ui::theme::theme;

use super::session_bars::{BarKind, DayBar};

pub(super) fn render_kpi_bar(
    frame: &mut Frame,
    area: Rect,
    bars: &[DayBar],
    hits: &[&RateLimitHit],
) {
    let t = theme();

    // KPI 1: Avg max cost/session at 100% saturation (from history bars, excluding current)
    let history_bars: Vec<&DayBar> = bars
        .iter()
        .filter(|b| b.kind != BarKind::Current && b.cost() > 0.0)
        .collect();
    let avg_cost = if history_bars.is_empty() {
        0.0
    } else {
        let sum: f64 = history_bars.iter().map(|b| b.cost()).sum();
        sum / history_bars.len() as f64
    };
    let avg_str = format_cost(avg_cost);

    // KPI 2: Trend % (linear regression slope as % change over the period)
    let trend_str = if history_bars.len() >= 2 {
        let n = history_bars.len() as f64;
        let sum_x: f64 = (0..history_bars.len()).map(|i| i as f64).sum();
        let sum_y: f64 = history_bars.iter().map(|b| b.cost()).sum();
        let sum_xy: f64 = history_bars
            .iter()
            .enumerate()
            .map(|(i, b)| i as f64 * b.cost())
            .sum();
        let sum_x2: f64 = (0..history_bars.len()).map(|i| (i * i) as f64).sum();
        let denom = n * sum_x2 - sum_x * sum_x;
        if denom.abs() > 1e-9 {
            let slope = (n * sum_xy - sum_x * sum_y) / denom;
            let mean_y = sum_y / n;
            if mean_y > 0.0 {
                let pct = (slope / mean_y) * 100.0;
                if pct >= 0.0 {
                    format!("+{:.0}%", pct)
                } else {
                    format!("-{:.0}%", pct.abs())
                }
            } else {
                "—".to_string()
            }
        } else {
            "—".to_string()
        }
    } else {
        "—".to_string()
    };
    let trend_color = if trend_str.starts_with('+') {
        t.lines_positive
    } else if trend_str.starts_with('-') {
        t.error
    } else {
        t.text_dim
    };

    // KPI 3: Last hit recency.
    let last_hit_str = if let Some(hit) = hits.iter().max_by_key(|h| h.timestamp) {
        let local_ts = hit.timestamp.with_timezone(&chrono::Local);
        let days_ago = (chrono::Local::now().date_naive() - local_ts.date_naive()).num_days();
        if days_ago <= 0 {
            crate::ui::i18n::t("today").to_string()
        } else if days_ago == 1 {
            crate::ui::i18n::t("1d ago").to_string()
        } else {
            format!("{}d ago", days_ago)
        }
    } else {
        crate::ui::i18n::t("never").to_string()
    };
    let last_hit_color = if last_hit_str == crate::ui::i18n::t("never") {
        t.lines_positive
    } else if last_hit_str == crate::ui::i18n::t("today") {
        t.error
    } else {
        t.warning
    };

    let values: [(&str, &'static str, Color); 3] = [
        (&avg_str, crate::ui::i18n::t(" Avg max $/session "), t.tokens_in),
        (&trend_str, crate::ui::i18n::t(" Trend "), trend_color),
        (&last_hit_str, crate::ui::i18n::t(" Last hit "), last_hit_color),
    ];

    let col_constraints: Vec<Constraint> = (0..3).map(|_| Constraint::Ratio(1, 3)).collect();
    let col_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(col_constraints)
        .split(area);

    for (i, col_area) in col_areas.iter().enumerate() {
        let (val, label, color) = &values[i];
        let block = Block::default()
            .title(Span::styled(*label, Style::default().fg(t.text_dim)))
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(t.border));
        let paragraph = Paragraph::new(Span::styled(
            *val,
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center)
        .block(block);
        frame.render_widget(paragraph, *col_area);
    }
}
