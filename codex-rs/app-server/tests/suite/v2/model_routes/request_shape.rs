use super::*;

#[tokio::test]
async fn model_router_preserves_native_redirect_policy() -> Result<()> {
    // Given a native OpenAI endpoint redirecting to another origin.
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let target = responses::start_mock_server().await;
    wiremock::Mock::given(wiremock::matchers::path("/v1/responses"))
        .respond_with(responses::sse_response(responses::sse(vec![
            responses::ev_completed("native-target"),
        ])))
        .mount(&target)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .respond_with(
            wiremock::ResponseTemplate::new(307)
                .insert_header("location", format!("{}/v1/responses", target.uri())),
        )
        .mount(&openai)
        .await;
    let (_home, mut server) = router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams::default(),
        })
        .await?;

    // When sampling on the primary model rather than an explicit route.
    start_turn(
        &mut server,
        &started.thread.id,
        "keep the native redirect policy",
    )
    .await?;
    completed(&mut server).await?;

    // Then its pre-existing redirect behavior is unchanged and Kimi is untouched.
    assert_eq!(target.received_requests().await.unwrap().len(), 1);
    assert!(kimi.received_requests().await.unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn model_router_rejects_redirects_without_contacting_other_origins() -> Result<()> {
    for status in [301, 302, 303, 307, 308] {
        // Given a routed endpoint redirecting to a different mock origin.
        let openai = responses::start_mock_server().await;
        let kimi = responses::start_mock_server().await;
        let target = responses::start_mock_server().await;
        wiremock::Mock::given(wiremock::matchers::path("/v1/responses"))
            .respond_with(responses::sse_response(responses::sse(vec![
                responses::ev_completed("redirect-target"),
            ])))
            .mount(&target)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/responses"))
            .respond_with(
                wiremock::ResponseTemplate::new(status)
                    .insert_header("retry-after", "0")
                    .insert_header("location", format!("{}/v1/responses", target.uri())),
            )
            .mount(&kimi)
            .await;
        let (_home, mut server) =
            router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
        let started: ThreadStartResponse = server
            .request(|request_id| ClientRequest::ThreadStart {
                request_id,
                params: ThreadStartParams {
                    model: Some("k3.1-pro".into()),
                    ..Default::default()
                },
            })
            .await?;

        // When the selected routed model encounters the redirect.
        start_turn(
            &mut server,
            &started.thread.id,
            "stay on the configured endpoint",
        )
        .await?;
        let finished: TurnCompletedNotification =
            timeout(TIMEOUT, server.read_notification("turn/completed")).await??;

        // Then neither redirect following nor provider fallback can move the request.
        assert!(
            target.received_requests().await.unwrap().is_empty(),
            "followed HTTP {status} redirect"
        );
        assert!(
            openai
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| !r.url.path().ends_with("/responses"))
        );
        assert_eq!(finished.turn.status, TurnStatus::Failed, "HTTP {status}");
        assert!(finished.turn.error.is_some());
        let requests = kimi.received_requests().await.unwrap();
        assert!(
            (1..=6).contains(&requests.len()),
            "bounded routed retries for HTTP {status}"
        );
        for request in requests {
            assert_eq!(
                request.headers.get("authorization").unwrap(),
                "Bearer kimi-only-test-key"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn model_router_normalizes_responses_lite_metadata_before_building_routed_requests()
-> Result<()> {
    // Given route metadata opting into the OpenAI-internal Responses Lite dialect.
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let routed =
        responses::mount_sse_once(&kimi, responses::sse(vec![responses::ev_completed("kimi")]))
            .await;
    let (home, mut server) = router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
    server.shutdown_gracefully().await?;
    let path = home.path().join("model-routes.json");
    let mut catalog: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    catalog["routes"][0]["models"][0]["use_responses_lite"] = json!(true);
    catalog["routes"][0]["models"][0]["tool_mode"] = json!("direct");
    std::fs::write(path, serde_json::to_vec(&catalog)?)?;
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
            params: ThreadStartParams {
                model: Some("k3.1-pro".into()),
                dynamic_tools: Some(vec![tool_round_trip::tool_spec()]),
                ..Default::default()
            },
        })
        .await?;

    // When sampling with the routed model and an actual function declaration.
    start_turn(
        &mut server,
        &started.thread.id,
        "use standard Responses on Kimi",
    )
    .await?;
    completed(&mut server).await?;

    // Then both headers and payload use standard Responses, not merely a hidden Lite header.
    let request = routed.single_request();
    let body = request.body_json();
    assert!(body["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["type"] == "function" && tool["name"] == "router_echo")
    }));
    assert!(
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] != "additional_tools")
    );
    assert!(body["reasoning"].get("context").is_none());
    assert_eq!(body["model"], "k3.1-pro");
    let requests = kimi.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .headers
            .get("x-openai-internal-codex-responses-lite")
            .is_none()
    );
    assert!(
        openai
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| !r.url.path().ends_with("/responses"))
    );
    Ok(())
}
