//! Model-family presentation at constrained sizes and with configured list bindings.

use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn model_picker_compact_hint_and_actions_follow_configured_bindings() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.5")).await;
    let mut keymap = crate::keymap::RuntimeKeymap::defaults();
    keymap.list.accept = vec![key_hint::plain(KeyCode::F(3))];
    keymap.list.cancel = vec![key_hint::plain(KeyCode::F(2))];
    chat.bottom_pane.set_keymap_bindings(&keymap);
    chat.open_all_models_popup();
    let popup = render_bottom_popup(&chat, /*width*/ 40);
    assert_eq!(popup.lines().last().unwrap().trim(), "f3 select · f2 back");
    chat.handle_key_event(KeyEvent::from(KeyCode::F(3)));
    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenReasoningPopup { .. }));
    chat.handle_key_event(KeyEvent::from(KeyCode::F(2)));
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn model_switcher_overlays_busy_chat_preserves_draft_and_never_interrupts() {
    let (mut chat, mut events, mut ops) = make_chatwidget_manual(Some("gpt-5.5")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.apply_external_edit("MODEL-SWITCHER-DRAFT".into());
    chat.on_task_started();
    while events.try_recv().is_ok() {}
    while ops.try_recv().is_ok() {}
    chat.open_model_switcher();
    assert!(chat.centered_dialog().is_some());
    assert_eq!(
        chat.model_popup_model_ids.len(),
        chat.model_catalog
            .try_list_models()
            .unwrap()
            .iter()
            .filter(|model| model.show_in_picker)
            .count()
    );
    assert!(chat.is_agent_turn_running());
    let request = chat.model_popup_request_id.unwrap();
    let mut presets = chat.model_catalog.try_list_models().unwrap();
    presets[0].display_name.push_str(" refreshed");
    for key in "no-model-matches-this".chars() {
        chat.handle_key_event(KeyCode::Char(key).into());
    }
    assert!(render_bottom_popup(&chat, 100).contains("no-model-matches-this"));
    assert!(chat.on_models_loaded(request, Ok(presets.clone())));
    assert!(chat.centered_dialog().is_some());
    assert!(render_bottom_popup(&chat, 100).contains("no-model-matches-this"));
    chat.handle_key_event(KeyCode::Esc.into());
    chat.handle_key_event(KeyCode::Esc.into());
    assert!(chat.no_modal_or_popup_active());
    assert_eq!(chat.composer_text_with_pending(), "MODEL-SWITCHER-DRAFT");
    assert!(chat.is_agent_turn_running());
    assert!(
        ops.try_recv().is_err(),
        "opening/searching/cancelling must not send an operation"
    );
    // The normal slash-command picker retains its original inline presentation.
    chat.open_model_popup();
    assert!(chat.centered_dialog().is_none());
    chat.handle_key_event(KeyCode::Esc.into());
    chat.open_model_switcher();
    chat.open_reasoning_popup(
        presets
            .into_iter()
            .find(|model| model.supported_reasoning_efforts.len() > 1)
            .unwrap(),
    );
    assert!(chat.centered_dialog().is_some());
    assert!(chat.is_agent_turn_running());
}

#[tokio::test]
async fn model_switcher_search_keeps_session_only_selection_on_a_control_key() {
    let (mut chat, mut events, mut ops) = make_chatwidget_manual(Some("gpt-5.5")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.on_task_started();
    chat.open_model_switcher();
    let preset = chat
        .model_catalog
        .try_list_models()
        .unwrap()
        .into_iter()
        .find(|model| model.model == "gpt-5.5")
        .unwrap();
    chat.open_reasoning_popup(preset);
    while events.try_recv().is_ok() {}
    while ops.try_recv().is_ok() {}
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| { matches!(event, AppEvent::SelectSessionModel { .. }) })
    );
    assert!(chat.is_agent_turn_running());
    assert!(ops.try_recv().is_err());
}
