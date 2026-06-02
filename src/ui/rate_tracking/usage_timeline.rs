use chrono::Timelike;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::data::models::format_tokens;
use crate::data::oauth::OAuthCredential;
use crate::data::tokens::MinuteTokens;
use crate::ui::theme::theme;

pub(super) fn render_usage_timeline(
    frame: &mut Frame,
    area: Rect,
    minute_tokens: &MinuteTokens,
    selected_cred: Option<&OAuthCredential>,
    _tick: usize,
) {
    let t = theme();
    let dot_color = t.tokens_in;

    let title = Line::from(vec![Span::styled(
        crate::ui::i18n::t(" Session token pace "),
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

    if inner.height < 2 || inner.width < 8 {
        return;
    }

    // Determine session window from the selected credential's five_hour.resets_at
    let now_local = chrono::Local::now();
    let (session_date, start_minute, now_minute) = if let Some(cred) = selected_cred
        && let Some(usage) = &cred.usage
        && let Some(five_h) = &usage.five_hour
        && let Some(resets_at_str) = &five_h.resets_at
        && let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str)
    {
        let session_start =
            (resets_at.with_timezone(&chrono::Local) - chrono::Duration::hours(5)).naive_local();
        let date = session_start.date();
        let sm = session_start.hour() as u16 * 60 + session_start.minute() as u16;
        let nm = if date == now_local.date_naive() {
            now_local.hour() as u16 * 60 + now_local.minute() as u16
        } else {
            // Session started yesterday — clamp to today
            now_local.hour() as u16 * 60 + now_local.minute() as u16
        };
        (date, sm, nm)
    } else {
        // Fallback: last 4 hours
        let nm = now_local.hour() as u16 * 60 + now_local.minute() as u16;
        let sm = nm.saturating_sub(nm.min(240));
        (now_local.date_naive(), sm, nm)
    };

    // Handle sessions that may span midnight
    let today = now_local.date_naive();
    let spans_midnight = session_date < today;
    let total_minutes = if spans_midnight {
        (1440 - start_minute) + now_minute
    } else {
        now_minute.saturating_sub(start_minute)
    };

    if total_minutes == 0 {
        return;
    }

    let bucket_min: u16 = 2;
    let n_buckets = total_minutes.div_ceil(bucket_min).max(1) as usize;

    let mut buckets = vec![0u64; n_buckets];
    for (&(date, minute), &val) in minute_tokens
        .input
        .iter()
        .chain(minute_tokens.output.iter())
    {
        let offset = if spans_midnight {
            if date == session_date && minute >= start_minute {
                Some((minute - start_minute) as u32)
            } else if date == today && minute <= now_minute {
                Some((1440 - start_minute) as u32 + minute as u32)
            } else {
                None
            }
        } else {
            if date == today && minute >= start_minute && minute <= now_minute {
                Some((minute - start_minute) as u32)
            } else {
                None
            }
        };
        if let Some(off) = offset {
            let idx = (off / bucket_min as u32) as usize;
            if idx < n_buckets {
                buckets[idx] += val;
            }
        }
    }

    let buckets = &buckets[..n_buckets];

    // Scale label (top-right)
    let max_val = buckets.iter().cloned().max().unwrap_or(0).max(1);
    let scale_label = format!("{}{}", format_tokens(max_val), crate::ui::i18n::t("/2m"));
    let scale_span = Span::styled(&scale_label, Style::default().fg(t.tokens_in));
    let scale_area = Rect::new(
        inner.x + inner.width.saturating_sub(scale_label.len() as u16),
        inner.y,
        scale_label.len() as u16,
        1,
    );
    frame.render_widget(Paragraph::new(scale_span), scale_area);

    let chart_h = inner.height.saturating_sub(1) as usize; // 1 row for x-axis
    let chart_w = inner.width as usize;
    if chart_w < 2 || chart_h < 1 {
        return;
    }

    // Always resample to exactly fill the available width (dot-columns = chart_w * 2)
    let dot_cols = chart_w * 2;
    let dot_rows = chart_h * 4;

    let data: Vec<f64> = {
        let ratio = n_buckets as f64 / dot_cols as f64;
        (0..dot_cols)
            .map(|i| {
                let center = (i as f64 + 0.5) * ratio;
                let lo = (center - ratio / 2.0).max(0.0).floor() as usize;
                let hi = (center + ratio / 2.0).ceil() as usize;
                let hi = hi.min(n_buckets);
                if lo >= hi {
                    let idx = center.round() as usize;
                    if idx < n_buckets {
                        buckets[idx] as f64
                    } else {
                        0.0
                    }
                } else {
                    let sum: u64 = buckets[lo..hi].iter().sum();
                    sum as f64 / (hi - lo) as f64
                }
            })
            .collect()
    };
    let n_points = data.len();
    let max_f = max_val as f64;

    // Braille line rendering — interpolate between consecutive points so the
    // curve is continuous, just like the dashboard sparkline.
    const DOT_BITS: [u8; 8] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];
    let mut braille_map: std::collections::HashMap<(u16, u16), u8> =
        std::collections::HashMap::new();

    let set_dot = |map: &mut std::collections::HashMap<(u16, u16), u8>, dx: usize, dy: usize| {
        let cell_x = dx / 2;
        let cell_y = dy / 4;
        let sub_x = dx % 2;
        let sub_y = dy % 4;
        let bit_idx = match (sub_x, sub_y) {
            (0, r @ 0..=2) => r,
            (0, 3) => 6,
            (1, r @ 0..=2) => r + 3,
            (1, 3) => 7,
            _ => unreachable!(),
        };
        let cx = inner.x + cell_x as u16;
        let cy = inner.y + cell_y as u16;
        if cx < inner.x + inner.width && cy < inner.y + chart_h as u16 {
            *map.entry((cx, cy)).or_default() |= DOT_BITS[bit_idx];
        }
    };

    // Convert data values to dot-row positions
    let y_positions: Vec<usize> = data
        .iter()
        .map(|&v| {
            let ratio = v / max_f;
            ((1.0 - ratio) * (dot_rows - 1) as f64).round() as usize
        })
        .collect();

    for dx in 0..n_points {
        let dy = y_positions[dx];
        set_dot(&mut braille_map, dx, dy);

        // Interpolate vertically between this point and the next
        if dx + 1 < n_points {
            let dy_next = y_positions[dx + 1];
            let (lo, hi) = if dy < dy_next {
                (dy, dy_next)
            } else {
                (dy_next, dy)
            };
            for y in lo..=hi {
                set_dot(&mut braille_map, dx, y);
            }
        }
    }

    let buf = frame.buffer_mut();
    for ((cx, cy), bits) in &braille_map {
        let ch = char::from_u32(0x2800 + *bits as u32).unwrap_or('·');
        let cell = &mut buf[(*cx, *cy)];
        cell.set_char(ch);
        cell.set_fg(dot_color);
    }

    // X-axis: time labels
    let x_row = inner.y + inner.height - 1;
    let label_count = (chart_w / 12).max(2);
    for li in 0..label_count {
        let col = if label_count > 1 {
            li * (chart_w - 1) / (label_count - 1)
        } else {
            0
        };
        let offset_min = (col as f64 / chart_w as f64 * total_minutes as f64) as u16;
        let abs_minute = (start_minute + offset_min) % 1440;
        let label = format!("{:02}:{:02}", abs_minute / 60, abs_minute % 60);
        // Shift last label left so it doesn't get clipped
        let label_len = label.len() as u16;
        let lx = if inner.x + col as u16 + label_len > inner.x + inner.width {
            (inner.x + inner.width).saturating_sub(label_len)
        } else {
            inner.x + col as u16
        };
        for (ci, ch) in label.chars().enumerate() {
            let x = lx + ci as u16;
            if x < inner.x + inner.width && x_row < inner.y + inner.height {
                let cell = &mut buf[(x, x_row)];
                cell.set_char(ch);
                cell.set_fg(t.text_dim);
            }
        }
    }
}
