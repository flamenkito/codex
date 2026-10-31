//! Session-picker chrome shares its geometry with paging and rendering.
//! Decorative gaps collapse before the list loses room for two comfortable rows.

use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;

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
    let fixed_chrome_rows = 7;
    const MIN_LIST_ROWS: u16 = 6;
    let gaps = area
        .height
        .saturating_sub(fixed_chrome_rows + MIN_LIST_ROWS)
        .min(/*other*/ 3);
    let [header, _, toolbar, _, search, _, list, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(gaps >= 3)),
        Constraint::Length(1),
        Constraint::Length(u16::from(gaps >= 2)),
        Constraint::Length(1),
        Constraint::Length(u16::from(gaps >= 1)),
        Constraint::Min(area.height.saturating_sub(fixed_chrome_rows + gaps)),
        Constraint::Length(4),
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

fn panel(state: &PickerState, area: Rect) -> Rect {
    let width = area.width.min(104);
    let desired_height = (state.filtered_rows.len().min(15) * 2 + 12).clamp(16, 28) as u16;
    let height = area.height.min(desired_height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn content_area(state: &PickerState, area: Rect) -> Rect {
    if state.switcher.is_some() {
        panel(state, area).inner(ratatui::layout::Margin::new(1, 1))
    } else {
        area
    }
}

pub(super) fn list_area(state: &PickerState, area: Rect) -> Rect {
    areas(content_area(state, area)).list
}

pub(super) fn render(frame: &mut crate::custom_terminal::Frame, state: &PickerState) {
    let PickerAreas {
        header,
        toolbar,
        search,
        list,
        footer,
    } = areas(content_area(state, frame.area()));
    if state.switcher.is_some() {
        frame.render_widget_ref(
            &ratatui::widgets::Block::bordered().border_style(crate::style::secondary_text_style()),
            panel(state, frame.area()),
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
    frame.render_widget_ref(
        &Line::from(
            if state.switcher.is_some() {
                "Sessions"
            } else {
                state.action.title()
            }
            .bold(),
        ),
        chrome(header),
    );
    let toolbar = chrome(toolbar);
    frame.render_widget_ref(&toolbar_for_width(state, toolbar.width), toolbar);
    let search = chrome(search);
    if state.switcher.is_some() {
        super::switcher::render_search(frame, state, search);
    } else {
        frame.render_widget_ref(&search_line(state, search.width), search);
    }
    let list = Rect::new(
        list.x.saturating_add(/*rhs*/ 2),
        list.y,
        list_viewport_width(list.width),
        list.height,
    );
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
