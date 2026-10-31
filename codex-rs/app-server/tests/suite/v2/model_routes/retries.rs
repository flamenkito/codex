use super::*;

async fn retry_stays_on_kimi(first: wiremock::ResponseTemplate) -> Result<()> {
    // Given a transient routed failure followed by a successful response.
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let routed = responses::mount_response_sequence(
        &kimi,
        vec![
            first,
            responses::sse_response(responses::sse(vec![
                responses::ev_assistant_message("retried", "ROUTED-RETRY-RESULT"),
                responses::ev_completed("retried"),
            ])),
        ],
    )
    .await;
    let (_home, mut server) = router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams {
                model: Some("k3.1-pro".into()),
                ..Default::default()
            },
        })
        .await?;

    // When the turn retries its failed HTTP request or incomplete SSE stream.
    start_turn(&mut server, &started.thread.id, "retry only on Kimi").await?;
    completed(&mut server).await?;

    // Then the retry preserves the selected model, credentials and origin.
    let requests = routed.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request.body_json()["model"], "k3.1-pro");
    }
    let requests = kimi.received_requests().await.expect("routed requests");
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(
            request
                .headers
                .get("authorization")
                .expect("routed authorization"),
            "Bearer kimi-only-test-key"
        );
    }
    assert!(
        openai
            .received_requests()
            .await
            .expect("primary requests")
            .iter()
            .all(|r| !r.url.path().ends_with("/responses"))
    );
    Ok(())
}

#[tokio::test]
async fn model_router_429_fails_closed_without_openai_inference() -> Result<()> {
    // Given the provider's existing non-retryable HTTP 429 policy.
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let routed = responses::mount_response_sequence(
        &kimi,
        vec![
            wiremock::ResponseTemplate::new(429).set_body_json(json!({"error": {
                "message": "rate limit", "type": "rate_limit_error"
            }})),
        ],
    )
    .await;
    let (_home, mut server) = router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams {
                model: Some("k3.1-pro".into()),
                ..Default::default()
            },
        })
        .await?;

    // When the routed upstream rejects the request with 429.
    start_turn(
        &mut server,
        &started.thread.id,
        "never fall back on rate limits",
    )
    .await?;
    let finished: TurnCompletedNotification =
        timeout(TIMEOUT, server.read_notification("turn/completed")).await??;

    // Then the failure remains on Kimi without either retrying or falling back to OpenAI.
    assert_eq!(finished.turn.status, TurnStatus::Failed);
    assert_eq!(
        finished.turn.error.unwrap().codex_error_info,
        Some(CodexErrorInfo::ResponseTooManyFailedAttempts {
            http_status_code: Some(429)
        })
    );
    assert_eq!(routed.requests().len(), 1);
    assert_eq!(routed.single_request().body_json()["model"], "k3.1-pro");
    let requests = kimi.received_requests().await.expect("routed requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer kimi-only-test-key"
    );
    assert!(
        openai
            .received_requests()
            .await
            .expect("primary requests")
            .iter()
            .all(|r| !r.url.path().ends_with("/responses"))
    );
    Ok(())
}

#[tokio::test]
async fn model_router_retries_5xx_only_on_kimi() -> Result<()> {
    retry_stays_on_kimi(
        wiremock::ResponseTemplate::new(503)
            .insert_header("retry-after", "0")
            .set_body_json(
                json!({"error": {"message": "temporarily unavailable", "type": "server_error"}}),
            ),
    )
    .await
}

#[tokio::test]
async fn model_router_retries_dropped_sse_only_on_kimi() -> Result<()> {
    retry_stays_on_kimi(responses::sse_response(responses::sse(vec![
        responses::ev_response_created("dropped-before-terminal-event"),
    ])))
    .await
}

async fn exhausted_retries_fail_closed(
    failure: wiremock::ResponseTemplate,
    max_attempts: usize,
) -> Result<()> {
    // Given a routed upstream that fails every attempt, including every retry.
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .respond_with(failure)
        .mount(&kimi)
        .await;
    let (_home, mut server) = router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams {
                model: Some("k3.1-pro".into()),
                ..Default::default()
            },
        })
        .await?;

    // When the selected provider exhausts its existing retry budget.
    start_turn(
        &mut server,
        &started.thread.id,
        "fail on Kimi when retries are exhausted",
    )
    .await?;
    let finished: TurnCompletedNotification =
        timeout(TIMEOUT, server.read_notification("turn/completed")).await??;

    // Then every attempt stays on Kimi and the terminal failure cannot trigger OpenAI inference.
    assert_eq!(finished.turn.status, TurnStatus::Failed);
    assert!(finished.turn.error.is_some());
    let requests = kimi.received_requests().await.expect("routed requests");
    assert!((2..=max_attempts).contains(&requests.len()));
    for request in requests {
        assert_eq!(
            request.body_json::<serde_json::Value>()?["model"],
            "k3.1-pro"
        );
        assert_eq!(
            request
                .headers
                .get("authorization")
                .expect("routed authorization"),
            "Bearer kimi-only-test-key"
        );
    }
    assert!(
        openai
            .received_requests()
            .await
            .expect("primary requests")
            .iter()
            .all(|r| !r.url.path().ends_with("/responses"))
    );
    Ok(())
}

#[tokio::test]
async fn model_router_exhausted_5xx_retries_never_fall_back_to_openai() -> Result<()> {
    exhausted_retries_fail_closed(
        wiremock::ResponseTemplate::new(503)
            .insert_header("retry-after", "0")
            .set_body_json(json!({"error": {"message": "unavailable", "type": "server_error"}})),
        30,
    )
    .await
}

#[tokio::test]
async fn model_router_exhausted_dropped_sse_retries_never_fall_back_to_openai() -> Result<()> {
    exhausted_retries_fail_closed(
        responses::sse_response(responses::sse(vec![responses::ev_response_created(
            "always-dropped",
        )])),
        6,
    )
    .await
}
