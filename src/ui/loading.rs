use std::time::Duration;

use ratatui::{prelude::*, widgets::Paragraph};

use super::theme::{star_span, theme};

const LOGO: [&str; 6] = [
    "  ██████╗ ██████╗ ███╗   ███╗███████╗████████╗███████╗██████╗ ",
    " ██╔════╝██╔════╝ ████╗ ████║██╔════╝╚══██╔══╝██╔════╝██╔══██╗",
    " ██║     ██║      ██╔████╔██║█████╗     ██║   █████╗  ██████╔╝",
    " ██║     ██║      ██║╚██╔╝██║██╔══╝     ██║   ██╔══╝  ██╔══██╗",
    " ╚██████╗╚██████╗ ██║ ╚═╝ ██║███████╗   ██║   ███████╗██║  ██║",
    "  ╚═════╝ ╚═════╝ ╚═╝     ╚═╝╚══════╝   ╚═╝   ╚══════╝╚═╝  ╚═╝",
];

pub(crate) fn draw_loading(frame: &mut Frame, elapsed: Duration) {
    let t = theme();
    let area = frame.area();

    if area.height < 12 || area.width < 30 {
        draw_compact(frame, area, elapsed);
        return;
    }

    let mut lines: Vec<Line<'_>> = Vec::with_capacity(LOGO.len() + 6);

    for &logo_line in &LOGO {
        lines.push(Line::from(Span::styled(
            logo_line,
            Style::default().fg(Color::Rgb(218, 119, 86)),
        )));
    }

    lines.push(Line::raw(""));

    let star_tick = (elapsed.as_millis() / 150) as usize;
    let (star, star_style) = star_span(star_tick);
    lines.push(Line::from(vec![
        Span::styled(star, star_style),
        Span::styled(
            format!("  {}", crate::ui::i18n::t("Parsing session files")),
            Style::default().fg(Color::Rgb(200, 200, 205)),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![Span::styled(
        crate::ui::i18n::t("Press q to quit"),
        Style::default().fg(t.text_dim).add_modifier(Modifier::DIM),
    )]));

    let content_h = lines.len() as u16;
    let pad_top = area.height.saturating_sub(content_h) / 2;
    let mut padded: Vec<Line<'_>> = Vec::with_capacity(pad_top as usize + lines.len());
    for _ in 0..pad_top {
        padded.push(Line::raw(""));
    }
    padded.extend(lines);

    frame.render_widget(Paragraph::new(padded).alignment(Alignment::Center), area);
}

fn draw_compact(frame: &mut Frame, area: Rect, elapsed: Duration) {
    let t = theme();
    let star_tick = (elapsed.as_millis() / 150) as usize;
    let (star, star_style) = star_span(star_tick);

    let rows = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled(
            "CCMeter",
            Style::default().fg(t.title).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(star, star_style),
            Span::styled(
                format!("  {}", crate::ui::i18n::t("Parsing session files")),
                Style::default().fg(Color::Rgb(200, 200, 205)),
            ),
        ]))
        .alignment(Alignment::Center),
        rows[3],
    );
}
