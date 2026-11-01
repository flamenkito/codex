//! Session-picker chrome shares its geometry with paging and rendering.
//! Decorative gaps collapse before the list loses room for two comfortable rows.

use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use super::PickerState;
use super::list_viewport_width;
use super::render_list;
use super::render_picker_footer;
use super::render_transcript_loading_overlay;
use super::search_line;
use super::toolbar_for_width;

pub(super) struct PickerAreas {
    header: Rect,
    toolbar: Rect,
    search: Rect,
    pub(super) list: Rect,
    footer: Rect,
}

pub(super) fn areas(area: Rect) -> PickerAreas {
    areas_with_footer(area, 4, 6)
}

fn areas_with_footer(area: Rect, footer_height: u16, min_list_rows: u16) -> PickerAreas {
    let fixed_chrome_rows = 3 + footer_height;
    let gaps = area
        .height
        .saturating_sub(fixed_chrome_rows + min_list_rows)
        .min(/*other*/ 3);
    let [header, _, toolbar, _, search, _, list, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(gaps >= 3)),
        Constraint::Length(1),
        Constraint::Length(u16::from(gaps >= 2)),
        Constraint::Length(1),
        Constraint::Length(u16::from(gaps >= 1)),
        Constraint::Min(area.height.saturating_sub(fixed_chrome_rows + gaps)),
        Constraint::Length(footer_height),
    ])
    .areas(area);
    PickerAreas {
        header,
        toolbar,
        search,
        list,
        footer,
    }
}

pub(super) fn panel(state: &PickerState, area: Rect) -> Rect {
    let width = area
        .width
        .saturating_sub(/*rhs*/ 2)
        .min(88)
        .max(area.width.min(8));
    // Size from the scoped catalog, not the query result, so searching never
    // makes the dialog jump when the number of matches changes.
    let catalog_rows = state
        .all_rows
        .iter()
        .filter(|row| state.row_matches_filter(row))
        .count();
    // Eight chrome rows, plus the group heading and overflow indicator. Reserve
    // four actual sessions rather than counting those decorations as sessions.
    let desired_height = (catalog_rows.min(4) + 10).clamp(11, 14) as u16;
    let height = area.height.min(desired_height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height / 4).min(area.height - height),
        width,
        height,
    )
}

fn content_area(state: &PickerState, area: Rect) -> Rect {
    if state.switcher.is_some() {
        panel(state, area).inner(ratatui::layout::Margin::new(3, 1))
    } else {
        area
    }
}

pub(super) fn list_area(state: &PickerState, area: Rect) -> Rect {
    picker_areas(state, area).list
}

fn picker_areas(state: &PickerState, area: Rect) -> PickerAreas {
    let area = content_area(state, area);
    if state.switcher.is_some() {
        let [header, _, search, _, list, _, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        PickerAreas {
            header,
            toolbar: Rect::default(),
            search,
            // The list renderer's gutter places labels at the shared chrome column.
            list: Rect::new(
                list.x.saturating_sub(2),
                list.y,
                list.width.saturating_add(4),
                list.height,
            ),
            footer,
        }
    } else {
        areas(area)
    }
}

/// Copy only the visible intersection, including on a resize while the popup is open.
pub(super) fn render_background(
    frame: &mut crate::custom_terminal::Frame,
    background: &ratatui::buffer::Buffer,
) {
    let area = frame.area().intersection(background.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            frame.buffer[(x, y)] = background[(x, y)].clone();
        }
    }
}

pub(super) fn render(frame: &mut crate::custom_terminal::Frame, state: &PickerState) {
    if let Some(background) = &state.switcher_background {
        render_background(frame, background);
    }
    if state.switcher.is_some() {
        crate::bottom_pane::dim_switcher_backdrop(frame.area(), frame.buffer);
    }
    let PickerAreas {
        header,
        toolbar,
        search,
        list,
        footer,
    } = picker_areas(state, frame.area());
    if state.switcher.is_some() {
        let panel = panel(state, frame.area());
        frame.render_widget_ref(&ratatui::widgets::Clear, panel);
        frame.render_widget_ref(
            &ratatui::widgets::Block::default().style(crate::bottom_pane::switcher_surface_style()),
            panel,
        );
    }
    let chrome = |area: Rect| {
        Rect::new(
            area.x.saturating_add(/*rhs*/ 1),
            area.y,
            area.width.saturating_sub(/*rhs*/ 2),
            area.height,
        )
    };
    let header = chrome(header);
    if state.switcher.is_some() {
        let escape = "esc";
        let gap = header
            .width
            .saturating_sub(("Sessions".len() + escape.len()) as u16);
        frame.render_widget_ref(
            &Line::from(vec![
                "Sessions".bold(),
                Span::raw(" ".repeat(usize::from(gap))),
                Span::styled(escape, crate::bottom_pane::switcher_secondary_style()),
            ]),
            header,
        );
    } else {
        frame.render_widget_ref(&Line::from(state.action.title().bold()), header);
    }
    if state.switcher.is_none() {
        let toolbar = chrome(toolbar);
        frame.render_widget_ref(&toolbar_for_width(state, toolbar.width), toolbar);
    }
    let search = chrome(search);
    if state.switcher.is_some() {
        super::switcher::render_search(frame, state, search);
    } else {
        frame.render_widget_ref(&search_line(state, search.width), search);
    }
    let list = if state.switcher.is_some() {
        Rect::new(list.x, list.y, list.width, list.height)
    } else {
        Rect::new(
            list.x.saturating_add(/*rhs*/ 2),
            list.y,
            list_viewport_width(list.width),
            list.height,
        )
    };
    render_list(frame, list, state);
    if state.is_transcript_loading() {
        render_transcript_loading_overlay(frame, list);
    }
    if state.switcher.is_some() {
        let footer = chrome(footer);
        for (index, line) in super::switcher::footer(state, footer.width)
            .iter()
            .enumerate()
        {
            if index < usize::from(footer.height) {
                frame.render_widget_ref(
                    line,
                    Rect::new(footer.x, footer.y + index as u16, footer.width, 1),
                );
            }
        }
    } else {
        render_picker_footer(frame, footer, state, list.height);
    }
}
