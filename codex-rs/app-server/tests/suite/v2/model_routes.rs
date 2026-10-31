//! A routed model remains a model choice on the same thread, with isolated upstream state.
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::write_models_cache;
use codex_app_server_protocol::*;
use codex_protocol::openai_models::ModelVisibility;
use core_test_support::responses;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(30);

mod request_shape;
mod retries;
mod tool_round_trip;

async fn set_model(server: &mut TestAppServer, thread: &str, model: &str) -> Result<()> {
    let _: ThreadSettingsUpdateResponse = server
        .request(|request_id| ClientRequest::ThreadSettingsUpdate {
            request_id,
            params: ThreadSettingsUpdateParams {
                thread_id: thread.into(),
                model: Some(model.into()),
                ..Default::default()
            },
        })
        .await?;
    Ok(())
}

async fn start_turn(server: &mut TestAppServer, thread: &str, text: &str) -> Result<()> {
    let _: TurnStartResponse = server
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.into(),
                input: vec![UserInput::Text {
                    text: text.into(),
                    text_elements: vec![],
                }],
                ..Default::default()
            },
        })
        .await?;
    Ok(())
}

async fn completed(server: &mut TestAppServer) -> Result<()> {
    let notification: TurnCompletedNotification =
        timeout(TIMEOUT, server.read_notification("turn/completed")).await??;
    assert_eq!(
        notification.turn.status,
        TurnStatus::Completed,
        "{:?}",
        notification.turn.error
    );
    Ok(())
}

fn write_route_catalog(home: &std::path::Path, kimi: &wiremock::MockServer) -> Result<()> {
    let mut routed = codex_models_manager::bundled_models_response()?
        .models
        .remove(0);
    routed.slug = "k3.1-pro".into();
    routed.display_name = "Kimi K3.1 Pro".into();
    routed.visibility = ModelVisibility::List;
    routed.supported_in_api = true;
    routed.use_responses_lite = false;
    std::fs::write(
        home.join("model-routes.json"),
        serde_json::to_vec(&json!({"routes": [{
            "provider": {"name": "Kimi", "base_url": format!("{}/v1", kimi.uri()), "env_key": "KIMI_ROUTER_TEST_KEY"},
            "models": [routed]
        }]}))?,
    )?;
    Ok(())
}

async fn router_fixture(
    openai: &wiremock::MockServer,
    kimi: &wiremock::MockServer,
    kimi_key: Option<&str>,
) -> Result<(TempDir, TestAppServer)> {
    let home = TempDir::new()?;
    MockResponsesConfig::new(&openai.uri())
        .with_provider_name("OpenAI")
        .with_provider_config("env_key = \"OPENAI_API_KEY\"\nsupports_websockets = false")
        .with_root_config("model_auto_compact_token_limit = 200000")
        .write(home.path())?;
    write_route_catalog(home.path(), kimi)?;
    write_models_cache(home.path()).await?;
    let server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[
            ("OPENAI_API_KEY", Some("openai-only-test-key")),
            ("KIMI_ROUTER_TEST_KEY", kimi_key),
        ])
        .build_initialized_with_timeout(TIMEOUT)
        .await?;
    Ok((home, server))
}

#[tokio::test]
async fn model_router_preserves_busy_turn_and_replays_conversation_across_upstreams() -> Result<()>
{
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let mut root_reasoning = responses::ev_reasoning_item("root-thought", &["root thinking"], &[]);
    root_reasoning["item"]["encrypted_content"] = json!("OPENAI-OPAQUE-REASONING");
    let root_response = responses::sse_response(responses::sse(vec![
        responses::ev_response_created("openai-first"),
        root_reasoning,
        responses::ev_assistant_message("openai-message", "OPENAI-HISTORY-MARKER"),
        responses::ev_completed("openai-first"),
    ]))
    .set_delay(Duration::from_secs(2));
    let root_followup = responses::sse_response(responses::sse(vec![
        responses::ev_response_created("openai-return"),
        responses::ev_assistant_message("openai-return-message", "returned to OpenAI"),
        responses::ev_completed("openai-return"),
    ]));
    let _root_mock =
        responses::mount_response_sequence(&openai, vec![root_response, root_followup]).await;
    let mut kimi_reasoning = responses::ev_reasoning_item("kimi-thought", &["Kimi thinking"], &[]);
    kimi_reasoning["item"]["encrypted_content"] = json!("KIMI-OPAQUE-REASONING");
    let _kimi_mock = responses::mount_sse_sequence(
        &kimi,
        vec![
            responses::sse(vec![
                responses::ev_response_created("kimi-tool"),
                kimi_reasoning,
                responses::ev_function_call(
                    "kimi-plan-call",
                    "router_echo",
                    r#"{"value":"routing-token"}"#,
                ),
                responses::ev_completed("kimi-tool"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("kimi-first"),
                responses::ev_message_item_added("kimi-message", ""),
                responses::ev_output_text_delta("KIMI-HISTORY-MARKER"),
                responses::ev_assistant_message("kimi-message", "KIMI-HISTORY-MARKER"),
                responses::ev_completed("kimi-first"),
            ]),
        ],
    )
    .await;
    let (_home, mut server) = router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
    let listed: ModelListResponse = server
        .request(|request_id| ClientRequest::ModelList {
            request_id,
            params: ModelListParams::default(),
        })
        .await?;
    assert!(
        listed.data.iter().any(|model| model.model == "k3.1-pro"),
        "Kimi missing from merged picker catalog"
    );
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams {
                dynamic_tools: Some(vec![DynamicToolSpec::Function(DynamicToolFunctionSpec {
                    name: "router_echo".into(), description: "Deterministic router test tool".into(),
                    input_schema: json!({"type":"object", "properties":{"value":{"type":"string"}}, "required":["value"], "additionalProperties":false}),
                    defer_loading: false,
                })]),
                ..Default::default()
            },
        })
        .await?;
    let thread = started.thread.id;
    start_turn(&mut server, &thread, "FIRST-USER-HISTORY-MARKER").await?;
    timeout(
        TIMEOUT,
        server.read_stream_until_notification_message("turn/started"),
    )
    .await??;
    set_model(&mut server, &thread, "k3.1-pro").await?;
    completed(&mut server).await?;
    assert!(
        kimi.received_requests().await.unwrap().is_empty(),
        "Changing the model must not reroute the in-flight turn"
    );
    start_turn(&mut server, &thread, "continue on Kimi").await?;
    let request = timeout(TIMEOUT, server.read_stream_until_request_message()).await??;
    let ServerRequest::DynamicToolCall { request_id, params } = request else {
        panic!("Expected routed tool call");
    };
    assert_eq!(params.tool, "router_echo");
    assert_eq!(params.arguments, json!({"value":"routing-token"}));
    server
        .send_response(
            request_id,
            serde_json::to_value(DynamicToolCallResponse {
                content_items: vec![DynamicToolCallOutputContentItem::InputText {
                    text: "ROUTER-TOOL-OUTPUT".into(),
                }],
                success: true,
            })?,
        )
        .await?;
    completed(&mut server).await?;
    set_model(&mut server, &thread, "mock-model").await?;
    start_turn(&mut server, &thread, "continue on OpenAI").await?;
    completed(&mut server).await?;
    let root_requests = openai
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    let kimi_requests = kimi.received_requests().await.unwrap();
    assert_eq!(root_requests.len(), 2);
    assert_eq!(kimi_requests.len(), 2);
    assert_eq!(
        kimi_requests[0].headers.get("authorization").unwrap(),
        "Bearer kimi-only-test-key"
    );
    for request in &root_requests {
        assert_eq!(
            request.headers.get("authorization").unwrap(),
            "Bearer openai-only-test-key"
        );
    }
    let kimi_body = kimi_requests[0].body_json::<serde_json::Value>()?;
    assert_eq!(kimi_body["model"], "k3.1-pro");
    assert!(kimi_body.get("previous_response_id").is_none());
    assert!(
        kimi_body["input"]
            .to_string()
            .contains("FIRST-USER-HISTORY-MARKER")
    );
    assert!(
        kimi_body["input"]
            .to_string()
            .contains("OPENAI-HISTORY-MARKER")
    );
    assert!(
        !kimi_body["input"]
            .to_string()
            .contains("OPENAI-OPAQUE-REASONING")
    );
    let followup = kimi_requests[1].body_json::<serde_json::Value>()?;
    assert!(
        followup["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "function_call_output"
                && item["call_id"] == "kimi-plan-call")
    );
    assert!(
        followup["input"]
            .to_string()
            .contains("KIMI-OPAQUE-REASONING"),
        "Keep this provider's current reasoning for its tool loop"
    );
    assert!(followup["input"].to_string().contains("ROUTER-TOOL-OUTPUT"));
    let returned = root_requests[1].body_json::<serde_json::Value>()?;
    assert!(
        returned["input"]
            .to_string()
            .contains("KIMI-HISTORY-MARKER")
    );
    assert!(
        !returned["input"]
            .to_string()
            .contains("KIMI-OPAQUE-REASONING")
    );
    let read: ThreadReadResponse = server
        .request(|request_id| ClientRequest::ThreadRead {
            request_id,
            params: ThreadReadParams {
                thread_id: thread.clone(),
                include_turns: true,
            },
        })
        .await?;
    assert_eq!(read.thread.id, thread);
    assert_eq!(read.thread.model_provider, "mock_provider");
    Ok(())
}

#[tokio::test]
async fn model_router_failure_never_falls_back_to_openai_or_replaces_its_credentials() -> Result<()>
{
    for key in [None, Some("wrong-kimi-key")] {
        let openai = responses::start_mock_server().await;
        let kimi = responses::start_mock_server().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(401).set_body_json(json!({"error": {"message": "invalid routed credential", "type": "invalid_api_key"}})))
            .mount(&kimi).await;
        let _recover = responses::mount_sse_once(
            &openai,
            responses::sse(vec![
                responses::ev_response_created("recover"),
                responses::ev_assistant_message("recover-msg", "OpenAI still works"),
                responses::ev_completed("recover"),
            ]),
        )
        .await;
        let (_home, mut server) = router_fixture(&openai, &kimi, key).await?;
        let started: ThreadStartResponse = server
            .request(|request_id| ClientRequest::ThreadStart {
                request_id,
                params: ThreadStartParams::default(),
            })
            .await?;
        set_model(&mut server, &started.thread.id, "k3.1-pro").await?;
        start_turn(&mut server, &started.thread.id, "do not fall back").await?;
        let failed: TurnCompletedNotification =
            timeout(TIMEOUT, server.read_notification("turn/completed")).await??;
        assert_eq!(failed.turn.status, TurnStatus::Failed);
        let requests = openai.received_requests().await.unwrap();
        assert!(
            !requests
                .iter()
                .any(|request| request.url.path().ends_with("/responses")),
            "A routed auth failure must not use OpenAI"
        );
        let routed = kimi.received_requests().await.unwrap();
        if key.is_none() {
            assert!(routed.is_empty());
        }
        for request in routed {
            assert_eq!(
                request.headers.get("authorization").unwrap(),
                "Bearer wrong-kimi-key"
            );
        }
        set_model(&mut server, &started.thread.id, "mock-model").await?;
        start_turn(&mut server, &started.thread.id, "recover on primary").await?;
        completed(&mut server).await?;
        let requests = openai.received_requests().await.unwrap();
        let request = requests
            .iter()
            .find(|request| request.url.path().ends_with("/responses"))
            .unwrap();
        assert_eq!(
            request.headers.get("authorization").unwrap(),
            "Bearer openai-only-test-key"
        );
    }
    Ok(())
}

#[tokio::test]
async fn model_router_resumes_the_same_thread_without_rewriting_the_primary_profile() -> Result<()>
{
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let _responses = responses::mount_sse_sequence(
        &kimi,
        vec![
            responses::sse(vec![
                responses::ev_assistant_message("before", "BEFORE-RESTART-MARKER"),
                responses::ev_completed("before"),
            ]),
            responses::sse(vec![
                responses::ev_assistant_message("after", "resumed"),
                responses::ev_completed("after"),
            ]),
        ],
    )
    .await;
    let (home, mut server) = router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
    let config_before = std::fs::read(home.path().join("config.toml"))?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams::default(),
        })
        .await?;
    let thread = started.thread.id;
    set_model(&mut server, &thread, "k3.1-pro").await?;
    start_turn(&mut server, &thread, "before restart").await?;
    completed(&mut server).await?;
    server.shutdown_gracefully().await?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[
            ("OPENAI_API_KEY", Some("openai-only-test-key")),
            ("KIMI_ROUTER_TEST_KEY", Some("kimi-only-test-key")),
        ])
        .build_initialized_with_timeout(TIMEOUT)
        .await?;
    let resumed: ThreadResumeResponse = server
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: thread.clone(),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(resumed.thread.id, thread);
    assert_eq!(resumed.model, "k3.1-pro");
    start_turn(&mut server, &thread, "after restart").await?;
    completed(&mut server).await?;
    assert_eq!(
        std::fs::read(home.path().join("config.toml"))?,
        config_before
    );
    let requests = kimi.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].body_json::<serde_json::Value>()?["input"]
            .to_string()
            .contains("BEFORE-RESTART-MARKER")
    );
    assert!(
        !openai
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path().ends_with("/responses"))
    );
    Ok(())
}

#[tokio::test]
async fn model_router_converts_legacy_encrypted_context_and_preserves_chatgpt_auth() -> Result<()> {
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let _root = responses::mount_sse_sequence(&openai, vec![
        responses::sse(vec![responses::ev_assistant_message("seed", "legacy visible answer"), responses::ev_completed_with_tokens("seed", 120)]),
        responses::sse(vec![json!({"type":"response.output_item.done", "item":{"type":"compaction", "encrypted_content":"OPENAI-LEGACY-CIPHER"}}), responses::ev_completed_with_tokens("compact", 120)]),
        responses::sse(vec![responses::ev_assistant_message("portable", "PORTABLE-CONVERSATION-SUMMARY"), responses::ev_completed_with_tokens("portable", 120)]),
    ]).await;
    let _routed = responses::mount_sse_once(
        &kimi,
        responses::sse(vec![
            responses::ev_assistant_message("kimi", "Kimi continued legacy conversation"),
            responses::ev_completed("kimi"),
        ]),
    )
    .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&openai.uri())
        .with_provider_name("OpenAI")
        .with_provider_config("requires_openai_auth = true\nsupports_websockets = false")
        .with_root_config(
            "model_auto_compact_token_limit = 200000\ncompact_prompt = \"Make portable summary\"",
        )
        .write(home.path())?;
    app_test_support::write_chatgpt_auth(
        home.path(),
        app_test_support::ChatGptAuthFixture::new("chatgpt-only-test-token").plan_type("pro"),
        codex_config::types::AuthCredentialsStoreMode::File,
    )?;
    write_models_cache(home.path()).await?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[
            ("OPENAI_API_KEY", None),
            ("KIMI_ROUTER_TEST_KEY", Some("kimi-only-test-key")),
        ])
        .build_initialized_with_timeout(TIMEOUT)
        .await?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams::default(),
        })
        .await?;
    let thread = started.thread.id;
    start_turn(&mut server, &thread, "seed old conversation").await?;
    completed(&mut server).await?;
    let _: ThreadCompactStartResponse = server
        .request(|request_id| ClientRequest::ThreadCompactStart {
            request_id,
            params: ThreadCompactStartParams {
                thread_id: thread.clone(),
            },
        })
        .await?;
    completed(&mut server).await?;
    server.shutdown_gracefully().await?;
    write_route_catalog(home.path(), &kimi)?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[
            ("OPENAI_API_KEY", None),
            ("KIMI_ROUTER_TEST_KEY", Some("kimi-only-test-key")),
        ])
        .build_initialized_with_timeout(TIMEOUT)
        .await?;
    let _: ThreadResumeResponse = server
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: thread.clone(),
                ..Default::default()
            },
        })
        .await?;
    set_model(&mut server, &thread, "k3.1-pro").await?;
    start_turn(
        &mut server,
        &thread,
        "continue the compacted conversation on Kimi",
    )
    .await?;
    completed(&mut server).await?;
    let root = openai
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    assert_eq!(root.len(), 3);
    for request in &root {
        assert_eq!(
            request.headers.get("authorization").unwrap(),
            "Bearer chatgpt-only-test-token"
        );
    }
    let conversion = _root.requests()[2].body_json();
    assert!(
        conversion["input"]
            .to_string()
            .contains("OPENAI-LEGACY-CIPHER")
    );
    let routed = kimi.received_requests().await.unwrap();
    assert_eq!(routed.len(), 1);
    assert_eq!(
        routed[0].headers.get("authorization").unwrap(),
        "Bearer kimi-only-test-key"
    );
    assert!(routed[0].headers.get("content-encoding").is_none());
    assert!(routed[0].headers.get("chatgpt-account-id").is_none());
    assert!(
        routed[0]
            .headers
            .get("x-openai-account-routing-override")
            .is_none()
    );
    let body = routed[0].body_json::<serde_json::Value>()?;
    assert!(
        body["input"]
            .to_string()
            .contains("PORTABLE-CONVERSATION-SUMMARY")
    );
    assert!(!body["input"].to_string().contains("OPENAI-LEGACY-CIPHER"));
    Ok(())
}

#[tokio::test]
async fn model_router_keeps_primary_websockets_without_leaking_their_continuation_to_kimi()
-> Result<()> {
    let primary = responses::start_websocket_server_with_headers(vec![
        responses::WebSocketConnectionConfig {
            requests: vec![
                vec![
                    responses::ev_response_created("primary-warm"),
                    responses::ev_completed("primary-warm"),
                ],
                vec![
                    responses::ev_response_created("primary-ws"),
                    responses::ev_assistant_message("primary-message", "WS-PRIMARY-HISTORY"),
                    responses::ev_completed("primary-ws"),
                ],
            ],
            response_headers: vec![("x-codex-turn-state".into(), "PRIMARY-STICKY-STATE".into())],
            accept_delay: None,
            close_after_requests: false,
        },
    ])
    .await;
    let kimi = responses::start_mock_server().await;
    let _response = responses::mount_sse_once(
        &kimi,
        responses::sse(vec![
            responses::ev_assistant_message("kimi", "Kimi after websocket"),
            responses::ev_completed("kimi"),
        ]),
    )
    .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&primary.uri().replacen("ws://", "http://", 1))
        .with_provider_name("OpenAI")
        .with_provider_config("env_key = \"OPENAI_API_KEY\"\nsupports_websockets = true")
        .write(home.path())?;
    write_route_catalog(home.path(), &kimi)?;
    write_models_cache(home.path()).await?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[
            ("OPENAI_API_KEY", Some("openai-only-test-key")),
            ("KIMI_ROUTER_TEST_KEY", Some("kimi-only-test-key")),
        ])
        .build_initialized_with_timeout(TIMEOUT)
        .await?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams::default(),
        })
        .await?;
    let thread = started.thread.id;
    start_turn(&mut server, &thread, "start with native websocket").await?;
    completed(&mut server).await?;
    let requests = primary.single_connection();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].body_json()["previous_response_id"],
        "primary-warm"
    );
    assert_eq!(
        primary
            .single_handshake()
            .header("authorization")
            .as_deref(),
        Some("Bearer openai-only-test-key")
    );
    set_model(&mut server, &thread, "k3.1-pro").await?;
    start_turn(&mut server, &thread, "continue on routed HTTP").await?;
    completed(&mut server).await?;
    let requests = kimi.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request.headers.get("authorization").unwrap(),
        "Bearer kimi-only-test-key"
    );
    assert!(request.headers.get("x-codex-turn-state").is_none());
    let body = request.body_json::<serde_json::Value>()?;
    assert!(body.get("previous_response_id").is_none());
    assert!(!body.to_string().contains("primary-warm"));
    assert!(!body.to_string().contains("PRIMARY-STICKY-STATE"));
    assert!(body["input"].to_string().contains("WS-PRIMARY-HISTORY"));
    server.shutdown_gracefully().await?;
    primary.shutdown().await;
    Ok(())
}

async fn resumed_retired_model_fixture(
    openai: &wiremock::MockServer,
    kimi: &wiremock::MockServer,
) -> Result<(TempDir, TestAppServer, String)> {
    let home = TempDir::new()?;
    MockResponsesConfig::new(&openai.uri())
        .with_model("gpt-6.1-sol")
        .with_provider_name("OpenAI")
        .with_provider_config("requires_openai_auth = true\nsupports_websockets = false")
        .with_root_config(
            "model_auto_compact_token_limit = 200000\ncompact_prompt = \"Make portable summary\"",
        )
        .write(home.path())?;
    app_test_support::write_chatgpt_auth(
        home.path(),
        app_test_support::ChatGptAuthFixture::new("chatgpt-only-test-token").plan_type("pro"),
        codex_config::types::AuthCredentialsStoreMode::File,
    )?;
    let mut previous = codex_models_manager::bundled_models_response()?
        .models
        .remove(0);
    previous.slug = "gpt-6.1-sol".into();
    previous.use_responses_lite = false;
    let mut current = previous.clone();
    current.slug = "mock-model".into();
    current.comp_hash = Some("current-hash".into());
    previous.comp_hash = Some("legacy-hash".into());
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/v1/models"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(json!({"models": [previous.clone(), current.clone()]})),
        )
        .mount(openai)
        .await;
    app_test_support::write_models_cache_with_models(home.path(), vec![previous, current]).await?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[
            ("OPENAI_API_KEY", None),
            ("KIMI_ROUTER_TEST_KEY", Some("kimi-only-test-key")),
        ])
        .build_initialized_with_timeout(TIMEOUT)
        .await?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams::default(),
        })
        .await?;
    let thread = started.thread.id;
    start_turn(&mut server, &thread, "seed retired model conversation").await?;
    completed(&mut server).await?;
    let _: ThreadCompactStartResponse = server
        .request(|request_id| ClientRequest::ThreadCompactStart {
            request_id,
            params: ThreadCompactStartParams {
                thread_id: thread.clone(),
            },
        })
        .await?;
    completed(&mut server).await?;
    server.shutdown_gracefully().await?;
    write_route_catalog(home.path(), kimi)?;
    let route_path = home.path().join("model-routes.json");
    let mut catalog: serde_json::Value = serde_json::from_slice(&std::fs::read(&route_path)?)?;
    catalog["routes"][0]["models"][0]["comp_hash"] = json!("current-hash");
    std::fs::write(route_path, serde_json::to_vec(&catalog)?)?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[
            ("OPENAI_API_KEY", None),
            ("KIMI_ROUTER_TEST_KEY", Some("kimi-only-test-key")),
        ])
        .build_initialized_with_timeout(TIMEOUT)
        .await?;
    let resumed: ThreadResumeResponse = server
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: thread.clone(),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(resumed.model, "gpt-6.1-sol");
    Ok((home, server, thread))
}

fn retired_model_responses() -> Vec<wiremock::ResponseTemplate> {
    vec![
        responses::sse_response(responses::sse(vec![
            responses::ev_assistant_message("seed", "legacy visible answer"),
            responses::ev_completed_with_tokens("seed", 120),
        ])),
        responses::sse_response(responses::sse(vec![
            json!({"type":"response.output_item.done", "item":{"type":"compaction", "encrypted_content":"OPENAI-LEGACY-CIPHER"}}),
            responses::ev_completed_with_tokens("compact", 120),
        ])),
        wiremock::ResponseTemplate::new(404).set_body_json(json!({"error": {
            "message": "The model gpt-6.1-sol does not exist",
            "type": "invalid_request_error", "code": "model_not_found", "param": "model"
        }})),
    ]
}

#[tokio::test]
async fn model_router_recovers_retired_legacy_compaction_on_native_before_kimi() -> Result<()> {
    // Given a resumed ChatGPT thread with opaque history from a now-unavailable model.
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let mut sequence = retired_model_responses();
    sequence.extend([
        responses::sse_response(responses::sse(vec![
            responses::ev_assistant_message("portable", "PORTABLE-RECOVERED-SUMMARY"),
            responses::ev_completed_with_tokens("portable", 120),
        ])),
        responses::sse_response(responses::sse(vec![
            responses::ev_assistant_message("native", "NATIVE-RECOVERED-ANSWER"),
            responses::ev_completed_with_tokens("native", 120),
        ])),
    ]);
    let root = responses::mount_response_sequence(&openai, sequence).await;
    let routed = responses::mount_sse_once(
        &kimi,
        responses::sse(vec![
            responses::ev_assistant_message("kimi", "continued on Kimi"),
            responses::ev_completed("kimi"),
        ]),
    )
    .await;
    let (_home, mut server, thread) = resumed_retired_model_fixture(&openai, &kimi)
        .await
        .expect("resume retired legacy fixture");

    // When recovering on the supported native model before switching providers.
    set_model(&mut server, &thread, "mock-model").await?;
    start_turn(&mut server, &thread, "recover on native").await?;
    completed(&mut server).await?;
    set_model(&mut server, &thread, "k3.1-pro").await?;
    start_turn(&mut server, &thread, "continue on Kimi after recovery").await?;
    completed(&mut server).await?;

    // Then compaction alone retries natively and subsequent history is portable.
    let requests = root.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[2].body_json()["model"], "gpt-6.1-sol");
    for index in [2, 3] {
        assert!(
            requests[index].body_json()["input"]
                .to_string()
                .contains("OPENAI-LEGACY-CIPHER")
        );
    }
    for index in [3, 4] {
        assert_eq!(requests[index].body_json()["model"], "mock-model");
    }
    for request in openai
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().ends_with("/responses"))
    {
        assert_eq!(
            request.headers.get("authorization").unwrap(),
            "Bearer chatgpt-only-test-token"
        );
    }
    assert!(
        !requests[4].body_json()["input"]
            .to_string()
            .contains("OPENAI-LEGACY-CIPHER")
    );
    let requests = routed.requests();
    assert_eq!(requests.len(), 1);
    let body = requests[0].body_json();
    assert_eq!(body["model"], "k3.1-pro");
    assert!(
        body["input"]
            .to_string()
            .contains("PORTABLE-RECOVERED-SUMMARY")
    );
    assert!(
        body["input"]
            .to_string()
            .contains("NATIVE-RECOVERED-ANSWER")
    );
    assert!(!body.to_string().contains("OPENAI-LEGACY-CIPHER"));
    assert!(
        !body["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "compaction" || item["type"] == "context_compaction")
    );
    Ok(())
}

#[tokio::test]
async fn model_router_retired_legacy_compaction_fails_closed_when_kimi_selected_first() -> Result<()>
{
    // Given the same unavailable native compactor, with no recovery response mounted.
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let root = responses::mount_response_sequence(&openai, retired_model_responses()).await;
    let (_home, mut server, thread) = resumed_retired_model_fixture(&openai, &kimi)
        .await
        .expect("resume retired legacy fixture");

    // When selecting Kimi directly before native recovery.
    set_model(&mut server, &thread, "k3.1-pro").await?;
    start_turn(&mut server, &thread, "do not answer on another provider").await?;
    let failed: TurnCompletedNotification =
        timeout(TIMEOUT, server.read_notification("turn/completed")).await??;

    // Then the compaction failure is terminal, with neither opaque leakage nor answer fallback.
    assert_eq!(failed.turn.status, TurnStatus::Failed);
    assert_eq!(
        failed.turn.error.as_ref().unwrap().codex_error_info,
        Some(CodexErrorInfo::ModelNotFound)
    );
    let requests = root.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        openai
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().ends_with("/responses"))
            .count(),
        3
    );
    assert_eq!(requests[2].body_json()["model"], "gpt-6.1-sol");
    assert!(
        requests[2].body_json()["input"]
            .to_string()
            .contains("OPENAI-LEGACY-CIPHER")
    );
    assert!(kimi.received_requests().await.unwrap().is_empty());
    Ok(())
}
