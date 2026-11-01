//! Geometry and painting for the persistent workspace status panel.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

const WIDTH: u16 = 40;

pub(super) fn split(area: Rect) -> (Rect, Option<Rect>) {
    if area.width < 120 || area.height < 12 {
        return (area, None);
    }
    let width = WIDTH;
    let chat = Rect {
        width: area.width - width,
        ..area
    };
    let panel = Rect::new(chat.right(), area.y, width, area.height);
    (chat, Some(panel))
}

pub(super) fn render(area: Rect, buffer: &mut Buffer, lines: Vec<Line<'static>>) {
    let style = crate::bottom_pane::switcher_surface_style();
    ratatui::widgets::Block::default()
        .style(style)
        .render(area, buffer);
    let footer_start = lines
        .iter()
        .position(|line| line.to_string() == "Workspace")
        .unwrap_or(lines.len());
    let (body, footer) = lines.split_at(footer_start);
    let footer = footer.iter().skip(1).cloned().collect::<Vec<_>>();
    let width = area.width.saturating_sub(4);
    let footer_height = Paragraph::new(footer.clone())
        .wrap(Wrap { trim: false })
        .line_count(width) as u16;
    let footer_y = area.bottom().saturating_sub(footer_height + 1);
    Paragraph::new(footer)
        .style(style)
        .wrap(Wrap { trim: false })
        .render(
            Rect::new(area.x + 2, footer_y, width, footer_height),
            buffer,
        );
    let body = body.to_vec();
    Paragraph::new(body)
        .style(style)
        .wrap(Wrap { trim: false })
        .render(
            Rect::new(
                area.x + 2,
                area.y + 1,
                width,
                footer_y.saturating_sub(area.y + 2),
            ),
            buffer,
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_hides_on_narrow_and_short_terminals() {
        let area = Rect::new(0, 0, 140, 36);
        let (chat, sidebar) = split(area);
        assert_eq!(chat.width, 100);
        assert_eq!(sidebar, Some(Rect::new(100, 0, 40, 36)));
        assert!(split(Rect::new(0, 0, 119, 36)).1.is_none());
        assert!(split(Rect::new(0, 0, 120, 36)).1.is_some());
        assert!(split(Rect::new(0, 0, 140, 8)).1.is_none());
    }

    #[test]
    fn sidebar_footer_survives_long_body_and_paints_last_row() {
        let area = Rect::new(0, 0, 40, 20);
        let mut buffer = Buffer::empty(area);
        let mut lines = vec![Line::from("body"); 100];
        lines.extend([
            Line::from("Workspace"),
            Line::from("/workspace"),
            Line::from("Codex test"),
        ]);
        render(area, &mut buffer, lines);
        assert_eq!(buffer[(2, 18)].symbol(), "C");
        assert_eq!(
            buffer[(39, 19)].bg,
            crate::bottom_pane::switcher_surface_style()
                .bg
                .expect("surface background")
        );
    }

    #[test]
    fn sidebar_paint_preserves_transcript_and_clips_long_content() {
        let area = Rect::new(0, 0, 140, 16);
        let mut buffer = Buffer::empty(area);
        buffer[(5, 2)].set_symbol("X");
        let (_, panel) = split(area);
        render(
            panel.expect("wide terminal"),
            &mut buffer,
            vec![Line::from("very long workspace path ".repeat(30))],
        );
        assert_eq!(buffer[(5, 2)].symbol(), "X");
        assert_ne!(buffer[(100, 2)].symbol(), "│");
    }
}
