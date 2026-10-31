use super::*;
use crate::resume_picker::tests::make_row;

fn row(id: ThreadId, date: &str, title: &str) -> Row {
    let mut row = make_row(&id.to_string(), date, title);
    row.thread_id = Some(id);
    row
}

#[test]
fn pins_group_before_recency_and_slots_do_not_retarget_during_search() {
    let older = ThreadId::new();
    let newer = ThreadId::new();
    let mut switcher = SessionSwitcher::load(&None);
    switcher.toggle_pin(older).unwrap();
    let mut rows = vec![
        row(newer, "2026-10-31T12:00:00Z", "Recent"),
        row(older, "2026-10-01T12:00:00Z", "Pinned"),
    ];
    switcher.arrange(&mut rows, Utc::now());
    assert_eq!(rows[0].thread_id, Some(older));
    assert_eq!(switcher.slots, vec![older, newer]);
    switcher.arrange(&mut rows[1..], Utc::now());
    assert_eq!(switcher.slots, vec![older, newer]);
}

#[test]
fn persisted_pins_merge_changes_from_another_picker_and_keep_invalid_files_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pins.json");
    let mut first = SessionSwitcher::load(&Some(path.clone()));
    let mut second = SessionSwitcher::load(&Some(path.clone()));
    let one = ThreadId::new();
    let two = ThreadId::new();
    first.toggle_pin(one).unwrap();
    second.toggle_pin(two).unwrap();
    assert_eq!(
        SessionSwitcher::load(&Some(path.clone()))
            .preferences
            .pinned,
        vec![one.to_string(), two.to_string()]
    );
    std::fs::write(&path, "invalid json").unwrap();
    assert!(first.toggle_pin(one).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid json");
}

fn state() -> PickerState {
    let (sender, _receiver) = tokio::sync::broadcast::channel(1);
    let mut state = PickerState::new(
        crate::backend_id::BackendId::new(),
        FrameRequester::new(sender),
        Arc::new(|_| {}),
        ProviderFilter::All,
        true,
        None,
        SessionPickerAction::Resume,
    );
    state.switcher = Some(SessionSwitcher::load(&None));
    state
}

#[tokio::test]
async fn pin_keeps_selection_delete_is_double_confirmed_and_current_is_protected() {
    let mut state = state();
    let current = ThreadId::new();
    let other = ThreadId::new();
    state.launch_context = SessionPickerLaunchContext::ExistingSession {
        current_thread_id: Some(current),
    };
    state.all_rows = vec![
        row(current, "2026-10-31T12:00:00Z", "Current"),
        row(other, "2026-10-30T12:00:00Z", "Other"),
    ];
    state.apply_filter();
    state.selected = 1;
    state
        .handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert_eq!(state.filtered_rows[state.selected].thread_id, Some(other));
    state
        .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert!(!state.switcher.as_ref().unwrap().pending);
    state
        .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert!(state.switcher.as_ref().unwrap().pending);
    state.handle_switcher_completion(
        state.backend_id,
        Ok(Completion::Deleted { thread_id: other }),
    );
    assert!(
        !state
            .switcher
            .as_ref()
            .unwrap()
            .preferences
            .pinned
            .contains(&other.to_string())
    );
    state
        .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert!(!state.switcher.as_ref().unwrap().pending);
    assert!(state.inline_error.as_ref().unwrap().contains("Switch away"));
}

#[tokio::test]
async fn quick_slots_return_backend_owned_target_and_search_excludes_subagents() {
    let mut state = state();
    let id = ThreadId::new();
    let child = ThreadId::new();
    let mut child_row = row(child, "2026-10-31T12:00:00Z", "Child");
    child_row.switcher_meta.child = true;
    state.all_rows = vec![row(id, "2026-10-31T12:00:00Z", "Main"), child_row];
    state.apply_filter();
    assert_eq!(state.filtered_rows.len(), 1);
    state
        .handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    let selection = state
        .handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE))
        .await
        .unwrap();
    assert!(
        matches!(selection, Some(SessionSelection::Resume(target)) if target.thread_id == id && target.backend_id == state.backend_id)
    );
}

#[tokio::test]
async fn compact_rows_share_height_math_with_paging_and_fit_unicode_titles() {
    let mut state = state();
    let id = ThreadId::new();
    state.all_rows = vec![row(id, "2026-10-31T12:00:00Z", "界界界 long session title")];
    state.apply_filter();
    let rendered = lines(&state.filtered_rows[0], &state, true, 24);
    assert_eq!(rendered.len(), 2);
    assert!(rendered.iter().all(|line| line.width() <= 24));
    state.view_width = Some(24);
    assert_eq!(state.rendered_height_between(0, 0), rendered.len());
}

#[tokio::test]
async fn rename_supports_editor_keys_and_paste_without_changing_search() {
    let mut state = state();
    let id = ThreadId::new();
    state.all_rows = vec![row(id, "2026-10-31T12:00:00Z", "Original")];
    state.apply_filter();
    state
        .handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    state
        .handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    state
        .handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    state.handle_paste("New 界 title".into());
    assert_eq!(
        state
            .switcher
            .as_ref()
            .unwrap()
            .rename
            .as_ref()
            .unwrap()
            .1
            .text(),
        "New 界 title"
    );
    assert!(state.query.is_empty());
    state
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    assert!(state.switcher.as_ref().unwrap().pending);
    state.handle_switcher_completion(
        state.backend_id,
        Ok(Completion::Renamed {
            thread_id: id,
            name: "New 界 title".into(),
        }),
    );
    assert_eq!(state.filtered_rows[0].display_preview(), "New 界 title");
}

#[tokio::test]
async fn activity_refresh_preserves_selection_and_does_not_clear_pending_mutation() {
    let mut state = state();
    let id = ThreadId::new();
    state.all_rows = vec![row(id, "2026-10-31T12:00:00Z", "Session")];
    state.apply_filter();
    state.switcher.as_mut().unwrap().pending = true;
    state.handle_switcher_completion(
        state.backend_id,
        Ok(Completion::Statuses(vec![(
            id,
            RowMeta {
                child: false,
                activity: "working",
            },
        )])),
    );
    assert!(state.switcher.as_ref().unwrap().pending);
    assert_eq!(state.filtered_rows[state.selected].thread_id, Some(id));
    assert_eq!(state.filtered_rows[0].switcher_meta.activity, "working");
}

#[tokio::test]
async fn mutation_completion_cannot_rename_another_backends_copy() {
    let mut state = state();
    let id = ThreadId::new();
    let other_backend = crate::backend_id::BackendId::new();
    let local = row(id, "2026-10-31T12:00:00Z", "Local");
    let mut other = row(id, "2026-10-31T12:00:00Z", "Remote");
    other.backend_id = Some(other_backend);
    state.all_rows = vec![local, other];
    state.apply_filter();
    state.handle_switcher_completion(
        other_backend,
        Ok(Completion::Renamed {
            thread_id: id,
            name: "Remote renamed".into(),
        }),
    );
    assert_eq!(state.all_rows[0].display_preview(), "Local");
    assert_eq!(state.all_rows[1].display_preview(), "Remote renamed");
}
