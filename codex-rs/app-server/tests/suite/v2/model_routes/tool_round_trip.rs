use super::*;

pub(super) fn tool_spec() -> DynamicToolSpec {
    DynamicToolSpec::Function(DynamicToolFunctionSpec {
        name: "router_echo".into(),
        description: "Deterministic router test tool".into(),
        input_schema: json!({"type":"object", "properties":{"value":{"type":"string"}}, "required":["value"], "additionalProperties":false}),
        defer_loading: false,
    })
}

async fn reply_to_tool(
    server: &mut TestAppServer,
    request: ServerRequest,
    call_id: &str,
) -> Result<()> {
    let ServerRequest::DynamicToolCall { request_id, params } = request else {
        panic!("expected dynamic tool call");
    };
    assert_eq!(params.tool, "router_echo");
    assert_eq!(params.call_id, call_id);
    assert_eq!(params.arguments, json!({"value": call_id}));
    server
        .send_response(
            request_id,
            serde_json::to_value(DynamicToolCallResponse {
                content_items: vec![DynamicToolCallOutputContentItem::InputText {
                    text: format!("OUTPUT-{}", params.call_id),
                }],
                success: true,
            })?,
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn model_router_pins_busy_kimi_tool_loop_then_round_trips_native_streamed_tools() -> Result<()>
{
    // Given a Kimi tool loop and a native turn with a streamed function call.
    let openai = responses::start_mock_server().await;
    let kimi = responses::start_mock_server().await;
    let routed = responses::mount_sse_sequence(
        &kimi,
        vec![
            responses::sse(vec![
                responses::ev_function_call("kimi-call", "router_echo", r#"{"value":"kimi-call"}"#),
                responses::ev_completed("kimi-tool"),
            ]),
            responses::sse(vec![
                responses::ev_assistant_message("kimi-done", "KIMI-TOOL-FINISHED"),
                responses::ev_completed("kimi-done"),
            ]),
        ],
    )
    .await;
    let native = responses::mount_sse_sequence(&openai, vec![
        responses::sse(vec![
            responses::ev_assistant_message("native-seed", "NATIVE-BEFORE-KIMI"),
            responses::ev_completed("native-seed"),
        ]),
        responses::sse(vec![
            responses::ev_response_created("native-tool"),
            json!({"type":"response.output_item.added", "output_index":0, "item":{"type":"function_call", "id":"fc_native", "call_id":"native-call", "name":"router_echo", "arguments":""}}),
            json!({"type":"response.function_call_arguments.delta", "item_id":"fc_native", "output_index":0, "delta":"{\"value\":\"native-call\"}"}),
            responses::ev_function_call("native-call", "router_echo", r#"{"value":"native-call"}"#),
            responses::ev_completed("native-tool"),
        ]),
        responses::sse(vec![responses::ev_assistant_message("native-done", "NATIVE-TOOL-FINISHED"), responses::ev_completed("native-done")]),
    ]).await;
    let (_home, mut server) = router_fixture(&openai, &kimi, Some("kimi-only-test-key")).await?;
    let started: ThreadStartResponse = server
        .request(|request_id| ClientRequest::ThreadStart {
            request_id,
            params: ThreadStartParams {
                dynamic_tools: Some(vec![tool_spec()]),
                ..Default::default()
            },
        })
        .await?;
    let thread = started.thread.id;
    start_turn(&mut server, &thread, "seed native conversation").await?;
    completed(&mut server).await?;
    set_model(&mut server, &thread, "k3.1-pro").await?;
    start_turn(&mut server, &thread, "start Kimi tool loop").await?;
    let pending = timeout(TIMEOUT, server.read_stream_until_request_message()).await??;

    // When selecting native while Kimi is blocked on its tool result, then running the next turn.
    set_model(&mut server, &thread, "mock-model").await?;
    assert_eq!(native.requests().len(), 1);
    reply_to_tool(&mut server, pending, "kimi-call").await?;
    completed(&mut server).await?;
    assert_eq!(routed.requests().len(), 2);
    assert_eq!(native.requests().len(), 1);
    start_turn(&mut server, &thread, "next turn uses native tools").await?;
    let pending = timeout(TIMEOUT, server.read_stream_until_request_message()).await??;
    reply_to_tool(&mut server, pending, "native-call").await?;
    completed(&mut server).await?;

    // Then each provider receives its own tool output and only the next turn moves to native.
    for (log, model, call, seed_requests) in [
        (&routed, "k3.1-pro", "kimi-call", 0),
        (&native, "mock-model", "native-call", 1),
    ] {
        let requests = log.requests();
        let requests = &requests[seed_requests..];
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_eq!(request.body_json()["model"], model);
        }
        let followup = requests[1].body_json();
        assert!(followup["input"].as_array().unwrap().iter().any(|item| {
            item["type"] == "function_call_output"
                && item["call_id"] == call
                && item["output"]
                    .to_string()
                    .contains(&format!("OUTPUT-{call}"))
        }));
    }
    for (upstream, key, count) in [
        (&openai, "openai-only-test-key", 3),
        (&kimi, "kimi-only-test-key", 2),
    ] {
        let requests = upstream.received_requests().await.unwrap();
        assert_eq!(requests.len(), count);
        for request in requests {
            assert_eq!(
                request.headers.get("authorization").unwrap(),
                &format!("Bearer {key}")
            );
        }
    }
    Ok(())
}
