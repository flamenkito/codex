//! Find and transcript selection retain control keys ahead of configurable chord prefixes.

use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn selection_copy_cancels_a_pending_find_editor_chord() -> Result<()> {
    let mut app = crate::app::test_support::make_test_app().await;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    app.keymap = RuntimeKeymap::from_config(&serde_json::from_value(serde_json::json!({
        "editor": {"move_line_start": ["ctrl-p h", "ctrl-x h"], "move_up": []}
    }))?)
    .expect("valid editor keymap");
    app.transcript_cells = vec![Arc::new(history_cell::PlainHistoryCell::new(vec![
        "needle text".into(),
    ]))];
    app.transcript_view.begin_search();
    app.transcript_view.paste_search("needle");
    for key in [
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        KeyCode::Enter.into(),
        KeyCode::Esc.into(),
    ] {
        assert_eq!(app.route_key_chord_event(&mut tui, key), Some(key));
        assert!(!app.key_chord_matcher.is_pending());
    }
    app.render_owned_transcript(&mut tui, Size::new(/*width*/ 80, /*height*/ 24))?;
    assert_eq!(
        app.route_key_chord_event(
            &mut tui,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
        ),
        None
    );
    assert!(app.key_chord_matcher.is_pending());
    app.transcript_view.handle_key(
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL),
        &app.transcript_cells,
    );
    for _ in "needle text".chars() {
        app.transcript_view
            .handle_key(KeyCode::Right.into(), &app.transcript_cells);
    }
    let copy = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.route_key_chord_event(&mut tui, copy), Some(copy));
    assert!(!app.key_chord_matcher.is_pending());
    assert_eq!(
        app.transcript_view
            .selected_text(&app.transcript_cells)
            .as_deref(),
        Some("needle text")
    );
    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

#[tokio::test]
async fn session_and_model_switcher_chords_remain_available_during_a_turn() -> Result<()> {
    let mut app = crate::app::test_support::make_test_app().await;
    let mut server = Box::pin(crate::start_embedded_app_server_for_picker(&app.config)).await?;
    let started = server.start_thread(&app.config).await?;
    let thread_id = started.session.thread_id;
    app.enqueue_primary_thread_session(started.session, started.turns)
        .await?;
    app.chat_widget.apply_external_edit("BUSY-DRAFT".into());
    app.chat_widget.handle_server_notification(
        codex_app_server_protocol::ServerNotification::TurnStarted(
            codex_app_server_protocol::TurnStartedNotification {
                thread_id: thread_id.to_string(),
                turn: serde_json::from_value(serde_json::json!({
                    "id": "busy-turn", "items": [], "status": "inProgress"
                }))?,
            },
        ),
        None,
    );
    let mut tui = crate::tui::test_support::make_test_tui()?;
    assert!(app.chat_widget.is_agent_turn_running());
    assert!(app.app_keymap_shortcuts_available());
    for (completion, is_session) in [('l', true), ('m', false)] {
        assert_eq!(
            app.route_key_chord_event(
                &mut tui,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
            ),
            None
        );
        let routed = app
            .route_key_chord_event(&mut tui, KeyCode::Char(completion).into())
            .expect("completed chord");
        app.handle_key_event(&mut tui, &mut server, routed).await;
        if is_session {
            assert!(app.pending_open_session_switcher);
            assert!(app.pending_open_resume_picker);
        } else {
            assert!(app.chat_widget.centered_dialog().is_some());
        }
        assert!(app.chat_widget.is_agent_turn_running());
        assert_eq!(app.chat_widget.composer_text_with_pending(), "BUSY-DRAFT");
    }
    app.chat_widget.handle_key_event(KeyCode::Esc.into());
    Box::pin(app.handle_event(
        &mut tui,
        &mut server,
        AppEvent::SelectSessionModel {
            model: "gpt-5.6-terra".into(),
            effort: Some(codex_protocol::openai_models::ReasoningEffort::High),
            service_tier: None,
        },
    ))
    .await?;
    assert_eq!(app.chat_widget.current_model(), "gpt-5.6-terra");
    assert!(app.chat_widget.is_agent_turn_running());
    assert_eq!(app.chat_widget.composer_text_with_pending(), "BUSY-DRAFT");
    Ok(())
}
