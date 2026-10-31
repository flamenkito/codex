//! Exercises dialog layout independently of any production confirmation caller.

use super::*;
use pretty_assertions::assert_eq;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

struct TestPane {
    presentation: ViewPresentation,
    preferred_dialog_width: Option<u16>,
}

impl BottomPaneView for TestPane {
    fn presentation(&self) -> ViewPresentation {
        self.presentation
    }

    fn preferred_dialog_width(&self) -> Option<u16> {
        self.preferred_dialog_width
    }
}

impl Renderable for TestPane {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if self.presentation == ViewPresentation::Inline {
                    buf[(x, y)].set_symbol(".");
                } else if y == area.top() {
                    buf[(x, y)].set_symbol("D");
                }
            }
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        3
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        Some((area.x, area.y))
    }

    fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
        SetCursorStyle::SteadyBar
    }
}

#[test]
fn centered_dialog_retains_backdrop_and_routes_cursor() {
    let backdrop = TestPane {
        presentation: ViewPresentation::Inline,
        preferred_dialog_width: None,
    };
    let dialog = TestPane {
        presentation: super::super::SelectionViewParams::confirmation().presentation,
        preferred_dialog_width: None,
    };
    let overlay = DialogOverlay {
        backdrop: RenderableItem::Borrowed(&backdrop),
        dialog: CenteredView(&dialog),
    };
    let mut snapshots = Vec::new();
    for (width, height, expected_cursor) in [(80, 9, (4, 3)), (24, 7, (2, 2)), (12, 2, (2, 0))] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| overlay.render(frame.area(), frame.buffer_mut()))
            .unwrap();
        let area = Rect::new(/*x*/ 0, /*y*/ 0, width, height);
        assert_eq!(
            (overlay.cursor_pos(area), overlay.cursor_style(area)),
            (Some(expected_cursor), SetCursorStyle::SteadyBar)
        );
        let views: Vec<Box<dyn BottomPaneView>> = vec![
            Box::new(TestPane {
                presentation: ViewPresentation::Inline,
                preferred_dialog_width: None,
            }),
            Box::new(TestPane {
                presentation: ViewPresentation::Centered,
                preferred_dialog_width: None,
            }),
        ];
        let stack = ViewStack(&views);
        let mut stacked = Buffer::empty(area);
        stack.render(area, &mut stacked);
        assert_eq!(&stacked, terminal.backend().buffer());
        assert_eq!(stack.cursor_pos(area), overlay.cursor_pos(area));
        snapshots.push(format!("{width}x{height}\n{}", terminal.backend()));
    }
    insta::assert_snapshot!(snapshots.join("\n"));
}

#[test]
fn centered_dialog_uses_preferred_width_and_viewport_clamping() {
    let dialog = TestPane {
        presentation: ViewPresentation::Centered,
        preferred_dialog_width: Some(100),
    };

    for (viewport, expected) in [
        (Rect::new(0, 0, 120, 20), Rect::new(10, 8, 100, 3)),
        (Rect::new(0, 0, 90, 20), Rect::new(2, 8, 86, 3)),
        (Rect::new(0, 0, 6, 2), Rect::new(0, 0, 6, 2)),
    ] {
        assert_eq!(dialog_area(&dialog, viewport), expected);
    }
}

#[test]
fn centered_dialog_without_preference_keeps_72_column_cap() {
    let dialog = TestPane {
        presentation: ViewPresentation::Centered,
        preferred_dialog_width: None,
    };

    assert_eq!(
        dialog_area(&dialog, Rect::new(0, 0, 120, 20)),
        Rect::new(24, 8, 72, 3)
    );
}
