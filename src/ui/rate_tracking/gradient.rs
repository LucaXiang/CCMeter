use ratatui::prelude::*;

use crate::ui::theme::theme;

/// Interpolate color from green → yellow → orange → red based on position
/// within the filled portion. The bar transitions smoothly across its length.
pub(super) fn gradient_color(position_ratio: f64) -> Color {
    // 4-stop gradient: green(0.0) → yellow(0.33) → orange(0.66) → red(1.0)
    let (r, g, b) = if position_ratio < 0.33 {
        let t = position_ratio / 0.33;
        (
            80.0 + t * 140.0, // 80 → 220
            200.0 + t * 0.0,  // 200 → 200
            80.0 - t * 40.0,  // 80 → 40
        )
    } else if position_ratio < 0.66 {
        let t = (position_ratio - 0.33) / 0.33;
        (
            220.0,            // 220
            200.0 - t * 60.0, // 200 → 140
            40.0,             // 40
        )
    } else {
        let t = (position_ratio - 0.66) / 0.34;
        (
            220.0,            // 220
            140.0 - t * 90.0, // 140 → 50
            40.0 + t * 10.0,  // 40 → 50
        )
    };
    Color::Rgb(r as u8, g as u8, b as u8)
}

/// Render a gradient bar into a single-line Rect.
/// Label on the left (fixed width), bar fills the rest with per-cell gradient color.
pub(super) fn gradient_bar_line<'a>(
    total_width: u16,
    ratio: f64,
    label: &str,
    pct: f64,
    reset: &str,
) -> Line<'a> {
    let t = theme();

    let reset_part = if reset.is_empty() {
        String::new()
    } else {
        format!("~{}", reset)
    };
    let label_text = format!("{:<7}{:>3.0}% {:<12} ", label, pct, reset_part);
    let label_len = label_text.len();

    let bar_width = (total_width as usize).saturating_sub(label_len);
    if bar_width == 0 {
        return Line::from(Span::styled(
            label_text,
            Style::default().fg(t.text_primary),
        ));
    }

    let mut spans: Vec<Span<'a>> = Vec::with_capacity(bar_width + 2);
    spans.push(Span::styled(
        label_text,
        Style::default().fg(t.text_primary),
    ));

    let filled = (ratio * bar_width as f64) as usize;

    // Filled cells with gradient
    for i in 0..filled.min(bar_width) {
        let pos = if bar_width > 1 {
            i as f64 / (bar_width - 1) as f64
        } else {
            0.0
        };
        spans.push(Span::styled(" ", Style::default().bg(gradient_color(pos))));
    }

    // Empty remainder
    let empty_count = bar_width.saturating_sub(filled);
    if empty_count > 0 {
        spans.push(Span::styled(
            " ".repeat(empty_count),
            Style::default().bg(t.empty_bar),
        ));
    }

    Line::from(spans)
}

/// Color for a utilization percentage (used for extra usage text).
pub(super) fn util_color(pct: f64) -> Color {
    if pct >= 75.0 {
        Color::Rgb(220, 50, 50)
    } else if pct >= 50.0 {
        Color::Rgb(220, 140, 40)
    } else if pct >= 25.0 {
        Color::Rgb(220, 200, 40)
    } else {
        Color::Rgb(80, 200, 80)
    }
}
