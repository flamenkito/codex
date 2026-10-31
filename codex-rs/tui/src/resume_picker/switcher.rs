//! Compact session-switcher extension. The host owns catalog loading, keyboard
//! routing and thread transitions; this module owns presentation and local pins.
use super::*;
use codex_app_server_protocol::ThreadStatus;
use serde::Deserialize;
use serde::Serialize;
use std::io::Write;

fn selected_style() -> Style {
    crate::bottom_pane::switcher_selection_style()
}

/// Fit terminal cells, rather than counting Unicode graphemes as columns.
fn fit(text: &str, width: usize) -> String {
    crate::line_truncation::truncate_line_with_ellipsis_if_overflow(
        Line::from(text.to_owned()),
        width,
    )
    .to_string()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RowMeta {
    pub(super) child: bool,
    pub(super) activity: &'static str,
}

impl RowMeta {
    pub(super) fn from_thread(thread: &Thread) -> Self {
        Self {
            child: thread.parent_thread_id.is_some(),
            activity: match &thread.status {
                ThreadStatus::Active { active_flags } if !active_flags.is_empty() => "waiting",
                ThreadStatus::Active { .. } => "working",
                ThreadStatus::SystemError => "error",
                ThreadStatus::Idle | ThreadStatus::NotLoaded => "",
            },
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Preferences {
    #[serde(default)]
    pinned: Vec<String>,
}

pub(super) enum Operation {
    Refresh { ids: Vec<ThreadId> },
    Rename { thread_id: ThreadId, name: String },
    Delete { thread_id: ThreadId },
    ReadPins { ids: Vec<ThreadId> },
}

pub(super) enum Completion {
    Statuses(Vec<(ThreadId, RowMeta)>),
    Renamed { thread_id: ThreadId, name: String },
    Deleted { thread_id: ThreadId },
    Pins(Vec<Row>),
}

pub(super) struct SessionSwitcher {
    path: Option<PathBuf>,
    preferences: Preferences,
    storage_error: Option<String>,
    groups: HashSet<SeenRowKey>,
    slots: Vec<ThreadId>,
    leader_pending: bool,
    deleting: Option<ThreadId>,
    rename: Option<(ThreadId, crate::bottom_pane::TextArea)>,
    pending: bool,
    refreshing: usize,
    selection: Option<SessionSelection>,
}

impl SessionSwitcher {
    pub(super) fn paste_rename(&mut self, text: &str) -> bool {
        if let Some((_, editor)) = &mut self.rename {
            if let Some(text) = normalize_pasted_search_query(text) {
                editor.insert_str_at(editor.cursor(), &text);
            }
            return true;
        }
        false
    }
    pub(super) fn captures_key(&self, key: KeyEvent, keymap: &RuntimeKeymap) -> bool {
        self.rename.is_some()
            || self.leader_pending
            || keymap
                .chords
                .leader
                .iter()
                .any(|binding| binding.is_press(key))
    }
    pub(super) fn load(path: &Option<PathBuf>) -> Self {
        let loaded = path.as_deref().map(Self::read).transpose();
        let (preferences, storage_error) = match loaded {
            Ok(preferences) => (preferences.unwrap_or_default(), None),
            Err(error) => (
                Preferences::default(),
                Some(format!("Could not read session pins: {error}")),
            ),
        };
        Self {
            path: path.clone(),
            preferences,
            storage_error,
            groups: HashSet::new(),
            slots: Vec::new(),
            leader_pending: false,
            deleting: None,
            rename: None,
            pending: false,
            refreshing: 0,
            selection: None,
        }
    }

    fn read(path: &Path) -> std::io::Result<Preferences> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Preferences::default())
            }
            Err(error) => Err(error),
        }
    }

    fn toggle_pin(&mut self, id: ThreadId) -> std::io::Result<()> {
        self.update_pin(id, true)
    }

    fn update_pin(&mut self, id: ThreadId, toggle: bool) -> std::io::Result<()> {
        // Serialize read-modify-write across CLI instances, then replace atomically.
        let _lock = if let Some(path) = &self.path {
            let parent = path
                .parent()
                .ok_or_else(|| std::io::Error::other("Invalid pins path"))?;
            std::fs::create_dir_all(parent)?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path.with_extension("lock"))?;
            file.lock()?;
            Some(file)
        } else {
            None
        };
        let mut next = match &self.path {
            Some(path) => Self::read(path)?,
            None => self.preferences.clone(),
        };
        let key = id.to_string();
        if !toggle || next.pinned.contains(&key) {
            next.pinned.retain(|pin| pin != &key);
        } else if next.pinned.len() < 100 {
            next.pinned.push(key);
        } else {
            return Err(std::io::Error::other("At most 100 sessions can be pinned."));
        }
        if let Some(path) = &self.path {
            let parent = path
                .parent()
                .ok_or_else(|| std::io::Error::other("Invalid pins path"))?;
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer_pretty(&mut file, &next)?;
            file.flush()?;
            file.persist(path).map_err(std::io::Error::other)?;
        }
        self.preferences = next;
        self.storage_error = None;
        Ok(())
    }

    fn pin_rank(&self, row: &Row) -> Option<usize> {
        let id = row.thread_id?.to_string();
        self.preferences.pinned.iter().position(|pin| pin == &id)
    }

    fn group(&self, row: &Row, reference: DateTime<Utc>) -> String {
        if self.pin_rank(row).is_some() {
            return "Pinned".to_string();
        }
        let Some(date) = row.updated_at.or(row.created_at) else {
            return "Older".to_string();
        };
        let date = date.with_timezone(&chrono::Local).date_naive();
        let today = reference.with_timezone(&chrono::Local).date_naive();
        if date == today {
            "Today".to_string()
        } else if Some(date) == today.pred_opt() {
            "Yesterday".to_string()
        } else {
            date.format("%Y-%m-%d").to_string()
        }
    }

    pub(super) fn arrange(&mut self, rows: &mut [Row], reference: DateTime<Utc>) {
        rows.sort_by_key(|row| {
            (
                self.pin_rank(row).unwrap_or(usize::MAX),
                std::cmp::Reverse(row.updated_at.or(row.created_at)),
            )
        });
        self.groups.clear();
        let mut previous = None;
        for row in rows.iter() {
            let group = self.group(row, reference);
            if previous.as_ref() != Some(&group) {
                if let Some(key) = row.seen_key() {
                    self.groups.insert(key);
                }
                previous = Some(group);
            }
        }
        // Freeze slots for this dialog so paging/search cannot silently retarget a shortcut.
        if self.slots.is_empty() {
            self.slots = rows
                .iter()
                .filter_map(|row| row.thread_id)
                .take(9)
                .collect();
        }
    }
}

pub(super) fn local_switcher_path(state: &PickerState) -> Option<PathBuf> {
    state
        .view_persistence
        .as_ref()
        .map(|settings| settings.codex_home.join("session-switcher.json"))
}

impl PickerState {
    pub(super) fn take_switcher_selection(&mut self) -> Option<SessionSelection> {
        self.switcher.as_mut()?.selection.take()
    }

    fn current_switcher_thread(&self) -> Option<ThreadId> {
        match self.launch_context {
            SessionPickerLaunchContext::ExistingSession { current_thread_id } => current_thread_id,
            SessionPickerLaunchContext::Startup => None,
        }
    }

    pub(super) fn handle_switcher_key(&mut self, key: KeyEvent) -> Result<bool> {
        let previous_filter = self.filter_mode;
        let selected = self
            .filtered_rows
            .get(self.selected)
            .and_then(Row::seen_key);
        let Some(mut switcher) = self.switcher.take() else {
            return Ok(false);
        };
        let result = self.handle_switcher_key_inner(&mut switcher, key);
        self.switcher = Some(switcher);
        if self.filter_mode != previous_filter {
            self.request_switcher_pins();
        }
        if result.as_ref().is_ok_and(|handled| *handled) {
            self.apply_filter();
            if let Some(key) = selected
                && let Some(index) = self
                    .filtered_rows
                    .iter()
                    .position(|row| row.seen_key().as_ref() == Some(&key))
            {
                self.selected = index;
                self.ensure_selected_visible();
            }
            self.request_frame();
        }
        result
    }

    fn handle_switcher_key_inner(
        &mut self,
        switcher: &mut SessionSwitcher,
        key: KeyEvent,
    ) -> Result<bool> {
        if let Some((thread_id, editor)) = &mut switcher.rename {
            match key.code {
                KeyCode::Esc => switcher.rename = None,
                KeyCode::Enter => {
                    let name = editor.text().trim().to_string();
                    if name.is_empty() {
                        self.inline_error = Some("Session title cannot be empty.".into());
                    } else if name.chars().count() > 200 {
                        self.inline_error = Some("Use a title of at most 200 characters.".into());
                    } else {
                        let thread_id = *thread_id;
                        switcher.rename = None;
                        switcher.pending = true;
                        self.send_switcher_operation(Operation::Rename { thread_id, name });
                    }
                }
                _ => editor.input_with_keymap(key, &self.keymap.editor),
            }
            return Ok(true);
        }
        if matches!(
            key.code,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right
        ) && key.modifiers.is_empty()
        {
            self.change_focused_filter();
            return Ok(true);
        }
        // Compact presentation must not change /resume's saved density or open hidden previews.
        if key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('o' | 'e')) {
            return Ok(true);
        }
        let leader = self
            .keymap
            .chords
            .leader
            .iter()
            .any(|binding| binding.is_press(key));
        if switcher.leader_pending {
            switcher.leader_pending = false;
            if key.code == KeyCode::Char('f') && key.modifiers.is_empty() {
                self.change_focused_filter();
                return Ok(true);
            }
            if let KeyCode::Char(digit @ '1'..='9') = key.code
                && key.modifiers.is_empty()
                && let Some(id) = switcher.slots.get((digit as u8 - b'1') as usize)
                && let Some(row) = self.all_rows.iter().find(|row| row.thread_id == Some(*id))
            {
                switcher.selection = row.thread_id.map(|thread_id| {
                    self.action.selection(SessionTarget {
                        backend_id: row.backend_id.unwrap_or(self.backend_id),
                        path: row.path.clone(),
                        thread_id,
                        cwd: row.cwd.clone(),
                        history_mode: row.history_mode,
                    })
                });
            }
            if switcher.selection.is_none() && matches!(key.code, KeyCode::Char('1'..='9')) {
                self.inline_error = Some("That quick slot is unavailable in this listing.".into());
            }
            return Ok(true);
        }
        if leader {
            switcher.leader_pending = true;
            return Ok(true);
        }
        let delete = key.code == KeyCode::Char('d') && key.modifiers == KeyModifiers::CONTROL;
        if !delete {
            switcher.deleting = None;
        }
        if switcher.pending {
            return Ok(false);
        }
        let Some(row) = self.filtered_rows.get(self.selected) else {
            return Ok(false);
        };
        let Some(id) = row.thread_id else {
            return Ok(false);
        };
        if key.modifiers == KeyModifiers::CONTROL {
            match key.code {
                KeyCode::Char('f') => {
                    if let Err(error) = switcher.toggle_pin(id) {
                        self.inline_error = Some(format!("Could not save pins: {error}"));
                    }
                    return Ok(true);
                }
                KeyCode::Char('r') => {
                    let mut editor = crate::bottom_pane::TextArea::new_single_line();
                    editor.set_text_clearing_elements(row.display_preview());
                    switcher.rename = Some((id, editor));
                    return Ok(true);
                }
                KeyCode::Char('d') => {
                    if self.current_switcher_thread() == Some(id) {
                        self.inline_error =
                            Some("Switch away before deleting the current session.".into());
                    } else if switcher.deleting == Some(id) {
                        switcher.deleting = None;
                        switcher.pending = true;
                        self.send_switcher_operation(Operation::Delete { thread_id: id });
                    } else {
                        switcher.deleting = Some(id);
                    }
                    return Ok(true);
                }
                _ => {}
            }
        }
        Ok(false)
    }

    fn change_focused_filter(&mut self) {
        if self.filter_cwd.is_none() {
            self.inline_error =
                Some("This backend does not expose a working directory filter.".into());
            return;
        }
        self.filter_mode = if self.filter_mode == SessionFilterMode::All {
            SessionFilterMode::Cwd
        } else {
            SessionFilterMode::All
        };
        self.start_initial_load();
    }

    fn send_switcher_operation(&self, operation: Operation) {
        let thread_id = match &operation {
            Operation::Rename { thread_id, .. } | Operation::Delete { thread_id } => {
                Some(*thread_id)
            }
            Operation::ReadPins { .. } | Operation::Refresh { .. } => None,
        };
        let backend_id = thread_id
            .and_then(|id| self.all_rows.iter().find(|row| row.thread_id == Some(id)))
            .and_then(|row| row.backend_id)
            .unwrap_or(self.backend_id);
        (self.picker_loader)(PickerLoadRequest::Switcher {
            backend_id,
            operation,
        });
    }

    pub(super) fn refresh_switcher_statuses(&mut self) {
        let Some(switcher) = &mut self.switcher else {
            return;
        };
        if switcher.refreshing > 0 || switcher.pending {
            return;
        }
        let mut sources = HashMap::<crate::backend_id::BackendId, Vec<ThreadId>>::new();
        for row in self
            .filtered_rows
            .iter()
            .skip(self.scroll_top)
            .take(self.view_rows.unwrap_or(20).min(32))
        {
            if let Some(id) = row.thread_id {
                sources
                    .entry(row.backend_id.unwrap_or(self.backend_id))
                    .or_default()
                    .push(id);
            }
        }
        switcher.refreshing = sources.len();
        for (backend_id, ids) in sources {
            (self.picker_loader)(PickerLoadRequest::Switcher {
                backend_id,
                operation: Operation::Refresh { ids },
            });
        }
    }

    pub(super) fn request_switcher_pins(&self) {
        if let Some(switcher) = &self.switcher {
            let ids = switcher
                .preferences
                .pinned
                .iter()
                .filter_map(|id| ThreadId::from_string(id).ok())
                .take(100)
                .collect::<Vec<_>>();
            if !ids.is_empty() {
                self.send_switcher_operation(Operation::ReadPins { ids });
            }
        }
    }

    pub(super) fn handle_switcher_completion(
        &mut self,
        backend_id: crate::backend_id::BackendId,
        result: std::io::Result<Completion>,
    ) {
        let Some(switcher) = self.switcher.as_mut() else {
            return;
        };
        if !matches!(&result, Ok(Completion::Pins(_) | Completion::Statuses(_))) {
            switcher.pending = false;
        }
        match result {
            Ok(Completion::Statuses(statuses)) => {
                switcher.refreshing = switcher.refreshing.saturating_sub(1);
                for (id, meta) in statuses {
                    for row in &mut self.all_rows {
                        if row.thread_id == Some(id)
                            && row.backend_id.unwrap_or(self.backend_id) == backend_id
                        {
                            row.switcher_meta = meta.clone();
                        }
                    }
                }
            }
            Ok(Completion::Renamed { thread_id, name }) => {
                for row in &mut self.all_rows {
                    if row.thread_id == Some(thread_id)
                        && row.backend_id.unwrap_or(self.backend_id) == backend_id
                    {
                        row.thread_name = Some(name.clone());
                    }
                }
            }
            Ok(Completion::Deleted { thread_id }) => {
                if let Err(error) = switcher.update_pin(thread_id, false) {
                    self.inline_error =
                        Some(format!("Session deleted, but pin cleanup failed: {error}"));
                }
                self.all_rows.retain(|row| {
                    row.thread_id != Some(thread_id)
                        || row.backend_id.unwrap_or(self.backend_id) != backend_id
                });
                // Keep seen_rows as a tombstone for pages already in flight.
                // Preserve slot numbers; a deleted slot becomes unavailable until reopening.
                self.transcript_cells.remove(&thread_id);
                self.transcript_previews.remove(&thread_id);
            }
            Ok(Completion::Pins(rows)) => {
                for row in rows {
                    if let Some(cwd) = &row.cwd {
                        let matches = self.row_matches_local_cwd(&row);
                        self.local_cwd_matches.insert(cwd.clone(), matches);
                    }
                    if let Some(key) = row.seen_key()
                        && self.seen_rows.insert(key)
                    {
                        self.all_rows.push(row);
                    }
                }
            }
            Err(error) => {
                switcher.refreshing = 0;
                self.inline_error = Some(format!("Session action failed: {error}"));
            }
        }
        self.apply_filter();
        self.request_frame();
    }
}

pub(super) async fn execute(
    app_server: &mut AppServerSession,
    mutation_handle: &AppServerRequestHandle,
    operation: Operation,
) -> std::io::Result<Completion> {
    match operation {
        Operation::Refresh { ids } => {
            let mut statuses = Vec::new();
            for chunk in ids.chunks(8) {
                let replies = futures::future::join_all(chunk.iter().map(|id| async move {
                    let response = mutation_handle
                        .request_typed::<codex_app_server_protocol::ThreadReadResponse>(
                            ClientRequest::ThreadRead {
                                request_id: RequestId::String(format!(
                                    "switcher-status-{}",
                                    Uuid::new_v4()
                                )),
                                params: codex_app_server_protocol::ThreadReadParams {
                                    thread_id: id.to_string(),
                                    include_turns: false,
                                },
                            },
                        )
                        .await;
                    (
                        *id,
                        response
                            .ok()
                            .map(|response| RowMeta::from_thread(&response.thread))
                            .unwrap_or(RowMeta {
                                child: false,
                                activity: "unknown",
                            }),
                    )
                }))
                .await;
                statuses.extend(replies);
            }
            Ok(Completion::Statuses(statuses))
        }
        Operation::Rename { thread_id, name } => {
            mutation_handle
                .request_typed::<codex_app_server_protocol::ThreadSetNameResponse>(
                    ClientRequest::ThreadSetName {
                        request_id: RequestId::String(format!(
                            "switcher-rename-{}",
                            Uuid::new_v4()
                        )),
                        params: codex_app_server_protocol::ThreadSetNameParams {
                            thread_id: thread_id.to_string(),
                            name: name.clone(),
                        },
                    },
                )
                .await
                .map_err(std::io::Error::other)?;
            Ok(Completion::Renamed { thread_id, name })
        }
        Operation::Delete { thread_id } => {
            mutation_handle
                .request_typed::<codex_app_server_protocol::ThreadDeleteResponse>(
                    ClientRequest::ThreadDelete {
                        request_id: RequestId::String(format!(
                            "switcher-delete-{}",
                            Uuid::new_v4()
                        )),
                        params: codex_app_server_protocol::ThreadDeleteParams {
                            thread_id: thread_id.to_string(),
                        },
                    },
                )
                .await
                .map_err(std::io::Error::other)?;
            Ok(Completion::Deleted { thread_id })
        }
        Operation::ReadPins { ids } => {
            let mut rows = Vec::new();
            for id in ids {
                if let Ok(thread) = app_server.thread_read(id, false).await {
                    let history_mode = thread.history_mode;
                    if let Some(mut row) = row_from_app_server_thread(thread) {
                        row.backend_id = Some(app_server.backend_id());
                        row.history_mode = Some(history_mode);
                        rows.push(row);
                    }
                }
            }
            Ok(Completion::Pins(rows))
        }
    }
}

pub(super) fn lines(
    row: &Row,
    state: &PickerState,
    selected: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(switcher) = state.switcher.as_ref() else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let reference = state.relative_time_reference.unwrap_or_else(Utc::now);
    let key = row.seen_key();
    if key
        .as_ref()
        .is_some_and(|key| switcher.groups.contains(key))
        || state
            .filtered_rows
            .get(state.scroll_top)
            .and_then(Row::seen_key)
            == key
    {
        lines.push(Line::from(
            fit(&switcher.group(row, reference), usize::from(width))
                .cyan()
                .bold(),
        ));
    }
    let current = row.thread_id.is_some() && row.thread_id == state.current_switcher_thread();
    let indicator = [row.switcher_meta.activity]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let directory = if state.filter_mode == SessionFilterMode::All {
        row.cwd
            .as_ref()
            .and_then(|cwd| cwd.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let suffix = [indicator, directory]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let suffix = fit(&suffix, usize::from(width / 3));
    let title = if switcher.deleting == row.thread_id && row.thread_id.is_some() {
        "Press Ctrl+D again to delete (stops running work)"
    } else {
        row.display_preview()
    };
    let title = fit(title, usize::from(width).saturating_sub(suffix.width() + 5));
    let style = if selected {
        selected_style()
    } else {
        crate::bottom_pane::switcher_surface_style()
    };
    let marker = if current { "●" } else { " " };
    let line = Line::from(vec![
        Span::raw(format!(" {marker} ")),
        Span::raw(title),
        Span::raw("  "),
        suffix.set_style(secondary_text_style()),
    ]);
    lines.extend(apply_session_row_background(vec![line], style, width));
    lines
}

pub(super) fn render_search(
    frame: &mut crate::custom_terminal::Frame,
    state: &PickerState,
    area: Rect,
) {
    let Some(switcher) = state.switcher.as_ref() else {
        frame.render_widget_ref(&search_line(state, area.width), area);
        return;
    };
    if let Some((_, editor)) = &switcher.rename {
        frame.render_widget_ref(&Line::from("Rename: "), area);
        let input = Rect::new(
            area.x.saturating_add(8),
            area.y,
            area.width.saturating_sub(8),
            area.height,
        );
        frame.render_widget_ref(editor, input);
        if let Some(position) = editor.cursor_pos(input) {
            frame.set_cursor_position(position);
        }
    } else {
        let label = if state.query.is_empty() {
            "Search".to_string()
        } else {
            format!("Search: {}", state.query)
        };
        frame.render_widget_ref(&Line::from(label), area);
        let cursor_x = area
            .x
            .saturating_add(if state.query.is_empty() {
                0
            } else {
                ("Search: ".len() + state.query.width()) as u16
            })
            .min(area.right().saturating_sub(1));
        frame.set_cursor_position((cursor_x, area.y));
    }
}

pub(super) fn footer(state: &PickerState, width: u16) -> Vec<Line<'static>> {
    let Some(switcher) = state.switcher.as_ref() else {
        return Vec::new();
    };
    let actions = if switcher.pending {
        vec!["Saving…".bold()]
    } else if switcher.rename.is_some() {
        vec![
            "save ".into(),
            "enter".set_style(secondary_text_style()),
            "   cancel ".into(),
            "esc".set_style(secondary_text_style()),
        ]
    } else if switcher.leader_pending {
        vec![
            "directory ".into(),
            "f".set_style(secondary_text_style()),
            "   quick switch ".into(),
            "1–9".set_style(secondary_text_style()),
            "   cancel ".into(),
            "esc".set_style(secondary_text_style()),
        ]
    } else if width >= 58 {
        vec![
            "pin/unpin ".into(),
            "ctrl+f".set_style(secondary_text_style()),
            "  delete ".into(),
            "ctrl+d".set_style(secondary_text_style()),
            "  rename ".into(),
            "ctrl+r".set_style(secondary_text_style()),
        ]
    } else {
        vec![
            "switch ".into(),
            "enter".set_style(secondary_text_style()),
            "  pin ".into(),
            "ctrl+f".set_style(secondary_text_style()),
        ]
    };
    let message = state
        .inline_error
        .clone()
        .or_else(|| switcher.storage_error.clone())
        .unwrap_or_default();
    vec![
        Line::from(actions),
        Line::from(fit(&message, usize::from(width)))
            .style(crate::bottom_pane::switcher_surface_style()),
    ]
}

#[cfg(test)]
mod tests;
