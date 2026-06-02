use chrono::Datelike;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::data::models::format_cost;
use crate::ui::theme::theme;

use super::session_bars::{BarKind, DayBar};

/// Distribute `bar_h` rows across the three cost components, preserving
/// proportions and giving every nonzero component at least 1 row when the
/// bar is tall enough (≥3 rows). Returns `[output_h, input_h, cache_h]`
/// from bottom to top.
fn three_way_split(bar_h: usize, parts: [f64; 3]) -> [usize; 3] {
    if bar_h == 0 {
        return [0; 3];
    }
    let total: f64 = parts.iter().sum();
    if total <= 0.0 {
        return [0; 3];
    }
    // Initial proportional rounding.
    let bar_h_f = bar_h as f64;
    let mut h: [usize; 3] = [
        ((parts[0] / total) * bar_h_f).round() as usize,
        ((parts[1] / total) * bar_h_f).round() as usize,
        ((parts[2] / total) * bar_h_f).round() as usize,
    ];
    // Boost zeros to 1 for nonzero shares when there's room.
    if bar_h >= 3 {
        for k in 0..3 {
            if parts[k] > 0.0 && h[k] == 0 {
                h[k] = 1;
            }
        }
    }
    // Trim the largest until the sum fits.
    while h.iter().sum::<usize>() > bar_h {
        let max_idx = (0..3).max_by_key(|&k| h[k]).unwrap_or(0);
        if h[max_idx] == 0 {
            break;
        }
        h[max_idx] -= 1;
    }
    // Grow the part with the largest share until the sum reaches bar_h.
    while h.iter().sum::<usize>() < bar_h {
        let max_idx = (0..3)
            .max_by(|&a, &b| {
                parts[a]
                    .partial_cmp(&parts[b])
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        h[max_idx] += 1;
    }
    h
}

pub(super) fn render_session_chart(frame: &mut Frame, area: Rect, bars: &[DayBar], _tick: usize) {
    let t = theme();
    // Output gets the darkest shade because it's typically the dominant cost.
    let red = Color::Rgb(220, 60, 60);
    let red_mid = Color::Rgb(245, 120, 100);
    let red_light = Color::Rgb(255, 195, 175);

    let blue = Color::Rgb(80, 170, 240);
    let blue_mid = Color::Rgb(140, 205, 250);
    let blue_light = Color::Rgb(205, 230, 255);

    let violet = Color::Rgb(160, 100, 220);
    let violet_mid = Color::Rgb(200, 150, 240);
    let violet_light = Color::Rgb(235, 210, 255);

    let title = Line::from(vec![
        Span::styled(
            crate::ui::i18n::t(" Daily session cost "),
            Style::default()
                .fg(t.heatmap_title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("██", Style::default().fg(violet)),
        Span::styled(crate::ui::i18n::t(" history "), Style::default().fg(t.text_dim)),
        Span::styled("██", Style::default().fg(red)),
        Span::styled(crate::ui::i18n::t(" hit day "), Style::default().fg(t.text_dim)),
        Span::styled("██", Style::default().fg(blue)),
        Span::styled(
            crate::ui::i18n::t(" projected live "),
            Style::default().fg(t.text_dim),
        ),
        Span::styled("│ ", Style::default().fg(t.border)),
        Span::styled("█", Style::default().fg(violet_light)),
        Span::styled("█", Style::default().fg(violet_mid)),
        Span::styled("█", Style::default().fg(violet)),
        Span::styled(
            crate::ui::i18n::t(" cache/in/out "),
            Style::default().fg(t.text_dim),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if bars.is_empty() || inner.height < 4 || inner.width < 4 {
        if inner.height >= 1 && inner.width >= 4 {
            let msg = Paragraph::new(Span::styled(
                crate::ui::i18n::t("no data"),
                Style::default().fg(t.text_dim),
            ));
            frame.render_widget(msg, inner);
        }
        return;
    }

    // Empty row between title and chart
    let inner = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };

    // Reserve 1 row for date labels, 1 for scale label
    let chart_height = inner.height.saturating_sub(2) as usize;
    if chart_height == 0 {
        return;
    }

    let max_cost = bars
        .iter()
        .map(|b| b.cost())
        .fold(0.0_f64, f64::max)
        .max(f64::MIN_POSITIVE);

    let n = bars.len();
    // Distribute full width evenly: each slot = total_width / n
    // Bar fills slot minus 1 char gap (minimum bar width = 1)
    let slot_w = (inner.width as usize / n).max(2);
    let bar_w = (slot_w - 1).max(1) as u16;
    let max_bars = inner.width as usize / slot_w;

    let visible = &bars[bars.len().saturating_sub(max_bars)..];
    let total_chart_w = visible.len() * slot_w;
    let x_offset = (inner.width as usize).saturating_sub(total_chart_w) / 2;
    let buf = frame.buffer_mut();

    for (i, bar) in visible.iter().enumerate() {
        let x = inner.x + x_offset as u16 + (i * slot_w) as u16;
        if x + bar_w > inner.x + inner.width {
            break;
        }

        let bar_cost = bar.cost();
        let ratio = bar_cost / max_cost;
        let bar_h = (ratio * chart_height as f64)
            .round()
            .max(if bar_cost > 0.0 { 1.0 } else { 0.0 }) as usize;

        // 3-shade palette: output (darkest) → input (mid) → cache (lightest)
        let (color_output, color_input, color_cache) = match bar.kind {
            BarKind::Current => (blue, blue_mid, blue_light),
            BarKind::RateLimited => (red, red_mid, red_light),
            BarKind::History => (violet, violet_mid, violet_light),
        };

        // Heights are proportional to each side's USD cost, not token count.
        let bar_h_capped = bar_h.min(chart_height);
        let [out_h, in_h, _cache_h] = three_way_split(
            bar_h_capped,
            [bar.output_cost, bar.input_cost, bar.cache_cost],
        );

        for dy in 0..bar_h_capped {
            let y = inner.y + chart_height as u16 - 1 - dy as u16;
            let c = if dy < out_h {
                color_output
            } else if dy < out_h + in_h {
                color_input
            } else {
                color_cache
            };
            for dx in 0..bar_w {
                let cell = &mut buf[(x + dx, y)];
                cell.set_char(' ');
                cell.set_bg(c);
            }
        }

        // Date label (day number) — skip labels when bars are too narrow
        // A day label is 2 chars wide; show every Nth label so they don't overlap
        let label_step = if slot_w >= 3 {
            1
        } else {
            3_usize.div_ceil(slot_w)
        };
        let label_y = inner.y + chart_height as u16;
        if i % label_step == 0 {
            let label = format!("{:>2}", bar.date.day());
            for (ci, ch) in label.chars().enumerate() {
                let lx = x + ci as u16;
                if lx < inner.x + inner.width && label_y < inner.y + inner.height {
                    let cell = &mut buf[(lx, label_y)];
                    cell.set_char(ch);
                    cell.set_fg(t.text_dim);
                }
            }
        }

        // Month label on first bar and when month changes
        let show_month =
            i == 0 || (i > 0 && visible[i].date.month() != visible[i - 1].date.month());
        if show_month {
            let month_y = inner.y + chart_height as u16 + 1;
            if month_y < inner.y + inner.height {
                let month_label = bar.date.format("%b").to_string();
                for (ci, ch) in month_label.chars().enumerate() {
                    let lx = x + ci as u16;
                    if lx < inner.x + inner.width {
                        let cell = &mut buf[(lx, month_y)];
                        cell.set_char(ch);
                        cell.set_fg(t.text_secondary);
                    }
                }
            }
        }
    }

    // Scale label (top-right): show max cost value
    let scale_label = format_cost(max_cost);
    let scale_y = inner.y;
    let scale_x = inner.x + inner.width.saturating_sub(scale_label.len() as u16);
    for (ci, ch) in scale_label.chars().enumerate() {
        let lx = scale_x + ci as u16;
        if lx < inner.x + inner.width && scale_y < inner.y + inner.height {
            let cell = &mut buf[(lx, scale_y)];
            cell.set_char(ch);
            cell.set_fg(t.text_dim);
        }
    }

    let nonzero: Vec<(f64, f64)> = visible
        .iter()
        .enumerate()
        .filter(|(_, b)| b.cost() > 0.0 && b.kind != BarKind::Current)
        .map(|(i, b)| (i as f64, b.cost()))
        .collect();

    if nonzero.len() >= 2 && chart_height >= 2 {
        let max_val = max_cost;
        let nn = nonzero.len() as f64;
        let sum_x: f64 = nonzero.iter().map(|p| p.0).sum();
        let sum_y: f64 = nonzero.iter().map(|p| p.1).sum();
        let sum_xy: f64 = nonzero.iter().map(|p| p.0 * p.1).sum();
        let sum_x2: f64 = nonzero.iter().map(|p| p.0 * p.0).sum();
        let denom = nn * sum_x2 - sum_x * sum_x;
        let (slope, intercept) = if denom.abs() > 1e-9 {
            let s = (nn * sum_xy - sum_x * sum_y) / denom;
            let i = (sum_y - s * sum_x) / nn;
            (s, i)
        } else {
            (0.0, sum_y / nn)
        };

        let (sr, sg, sb) = t.star_base;
        let trend_color = Color::Rgb(sr, sg, sb);
        let buf = frame.buffer_mut();
        let chart_w = total_chart_w;
        let vn = visible.len();

        // Braille: each cell is 2x4 dots, giving sub-character resolution
        // Row of dots: 4 vertical per cell, so effective Y resolution = chart_height * 4
        // Col of dots: 2 horizontal per cell, so effective X resolution = chart_w * 2
        let dot_rows = chart_height * 4;
        let dot_cols = chart_w * 2;

        // Braille base: U+2800, dots indexed as:
        //  0 3
        //  1 4
        //  2 5
        //  6 7
        const DOT_BITS: [u8; 8] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];

        // Collect which braille dots to set per cell
        let mut braille_map: std::collections::HashMap<(u16, u16), u8> =
            std::collections::HashMap::new();

        for dx in 0..dot_cols {
            let data_x = if dot_cols > 1 {
                dx as f64 / (dot_cols - 1) as f64 * (vn - 1) as f64
            } else {
                0.0
            };
            let trend_val = slope * data_x + intercept;
            let ratio = trend_val / max_val;
            if !(0.0..=1.0).contains(&ratio) {
                continue;
            }
            let dy = ((1.0 - ratio) * (dot_rows - 1) as f64).round() as usize;

            let cell_x = dx / 2;
            let cell_y = dy / 4;
            let sub_x = dx % 2;
            let sub_y = dy % 4;
            // Braille dot order: col0=[0,1,2,6] col1=[3,4,5,7]
            let bit_idx = match (sub_x, sub_y) {
                (0, r @ 0..=2) => r,
                (0, 3) => 6,
                (1, r @ 0..=2) => r + 3,
                (1, 3) => 7,
                _ => unreachable!(),
            };

            let cx = inner.x + x_offset as u16 + cell_x as u16;
            let cy = inner.y + cell_y as u16;
            if cx < inner.x + inner.width && cy < inner.y + chart_height as u16 {
                *braille_map.entry((cx, cy)).or_default() |= DOT_BITS[bit_idx];
            }
        }

        for ((cx, cy), bits) in &braille_map {
            let ch = char::from_u32(0x2800 + *bits as u32).unwrap_or('·');
            let cell = &mut buf[(*cx, *cy)];
            cell.set_char(ch);
            cell.set_fg(trend_color);
        }
    }
}
