use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

pub(super) fn blinking_dot_span(tick: usize, color: Color) -> Span<'static> {
    let on = (tick / 7).is_multiple_of(2);
    if on {
        Span::styled(
            "● ",
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("  ")
    }
}

/// Human-readable label for a credential, derived directly from its source
/// root path (the last meaningful path segment). Independent of the source
/// selector so it stays correct in the provider view.
pub(super) fn credential_display_name(source_root: &str) -> &str {
    source_root
        .rsplit('/')
        .find(|s| !s.is_empty() && *s != "projects")
        .unwrap_or(source_root)
}

pub(super) fn source_color_index(source_root: &str, all_roots: &[String]) -> usize {
    all_roots.iter().position(|r| r == source_root).unwrap_or(0)
}

pub(super) fn format_rate_label(rate_per_min: f64) -> String {
    if rate_per_min >= 1000.0 {
        format!("{:.1}K/m", rate_per_min / 1000.0)
    } else {
        format!("{:.0}/m", rate_per_min)
    }
}

pub(super) fn format_duration_label(mins: f64) -> String {
    let mins = mins.max(0.0).round() as u64;
    if mins >= 24 * 60 {
        format!("{}d", mins / (24 * 60))
    } else if mins >= 60 {
        format!("{}h{:02}m", mins / 60, mins % 60)
    } else {
        format!("{}m", mins)
    }
}

pub(super) fn truncate_text(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

pub(super) fn format_reset(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%a %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
}
