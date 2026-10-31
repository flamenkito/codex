//! Turn-local inference routing. Primary login, account routing and socket state never move
//! into an independently authenticated provider. A route failure cannot retry on the primary.

use super::*;
use codex_model_provider_info::model_routes::ModelRoutes;
use codex_protocol::models::ContentItem;
use codex_tools::ToolSpec;
use std::path::Path;

impl ModelClient {
    pub(crate) fn with_model_routes_from_home(
        mut self,
        codex_home: &Path,
    ) -> std::io::Result<Self> {
        if self.state.provider.info().is_openai() {
            self.model_routes = Arc::new(ModelRoutes::load(codex_home)?);
        }
        Ok(self)
    }

    pub(super) fn prepare_portable_model_input(&self, input: &mut Vec<ResponseItem>) -> Result<()> {
        if self.model_routes.is_empty() {
            return Ok(());
        }
        let primary = self.state.provider.info().is_openai();
        if !primary
            && input.iter().any(|item| {
                matches!(
                    item,
                    ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. }
                )
            })
        {
            // Legacy encrypted compactions need reconstruction from the persisted rollout.
            // Failing here is preferable to losing history or sending another provider's opaque state.
            return Err(CodexErr::InvalidRequest("Rebuild portable conversation history before routing provider-specific compacted context".into()));
        }
        input.retain(|item| {
            reasoning_key(item).is_none_or(|key| {
                // Initial history may belong to another provider. Reasoning produced after this
                // turn's first request belongs to this upstream and is needed for its tool loop.
                self.portable_reasoning_prefix
                    .as_ref()
                    .is_some_and(|prefix| !prefix.contains(&key))
            })
        });
        if !primary {
            rewrite_legacy_custom_tool_history(input);
        }
        for item in input {
            if !matches!(
                item,
                ResponseItem::Compaction { .. }
                    | ResponseItem::ContextCompaction { .. }
                    | ResponseItem::Reasoning { .. }
            ) {
                item.clear_encrypted_content();
            }
            item.clear_internal_chat_message_metadata_passthrough();
            if let ResponseItem::FunctionCall {
                encrypted_function_args,
                ..
            } = item
            {
                *encrypted_function_args = None;
            }
        }
        Ok(())
    }

    pub(super) fn deduplicate_additional_tools(
        &self,
        prompt: &Prompt,
        input: &mut Vec<ResponseItem>,
    ) {
        if self.model_routes.is_empty() || self.state.provider.info().is_openai() {
            return;
        }
        deduplicate_additional_tool_declarations(input, &prompt.tools);
    }

    fn for_model_route(&self, provider_info: ModelProviderInfo) -> Self {
        let mut client = self.clone();
        let primary = &self.state;
        let workspace_routing =
            WorkspaceRoutingContext::new(provider_info.base_url.clone().unwrap_or_default());
        let provider = create_model_provider(provider_info, /*auth_manager*/ None);
        let auth_env_telemetry =
            collect_auth_env_telemetry(provider.info(), /*codex_api_key_env_enabled*/ false);
        client.state = Arc::new(ModelClientState {
            thread_id: primary.thread_id,
            provider,
            workspace_routing,
            auth_env_telemetry,
            session_source: primary.session_source.clone(),
            originator: primary.originator.clone(),
            model_verbosity: primary.model_verbosity,
            content_item_kinds_enabled: primary.content_item_kinds_enabled,
            reasoning_effort_override_enabled: false,
            enable_request_compression: false,
            include_timing_metrics: false,
            beta_features_header: None,
            concurrent_reasoning_summaries_enabled: false,
            output_token_replay_enabled: false,
            include_attestation: false,
            attestation_provider: None,
            disable_websockets: AtomicBool::new(true),
            agent_identity_session_fallback: AgentIdentitySessionFallback::default(),
            cached_websocket_session: StdMutex::new(WebsocketSession::default()),
            last_inference_tools: StdMutex::new(None),
        });
        client.runtime_auth = None;
        client.codex_responses_headers = None;
        client.request_contributors.clear();
        client.api_key_cyber_access_programs =
            cyber_access_program::ApiKeyCyberAccessPrograms::UnsupportedProvider;
        client
    }
}

impl ModelClientSession {
    pub(super) fn capture_portable_reasoning_prefix(&mut self, prompt: &Prompt) {
        if !self.client.model_routes.is_empty() && self.client.portable_reasoning_prefix.is_none() {
            self.client.portable_reasoning_prefix = Some(Arc::new(
                prompt.input.iter().filter_map(reasoning_key).collect(),
            ));
        }
    }

    pub(super) fn select_model_route(&mut self, model: &str) {
        let provider = self.primary_client.model_routes.provider_for_model(model);
        let route = provider.as_ref().map(|provider| {
            format!(
                "{}|{}|{}",
                provider.name,
                provider.base_url.as_deref().unwrap_or_default(),
                provider.env_key.as_deref().unwrap_or_default()
            )
        });
        if self.active_model_route == route {
            return;
        }
        // Prewarm may have connected the primary before the selected model was known.
        // Never send its delta, response ID, sticky token or socket to another provider.
        self.websocket_session.reset(Some("model_provider_changed"));
        self.turn_state = Arc::new(OnceLock::new());
        self.client = match provider {
            Some(provider) => self.primary_client.for_model_route(provider),
            None => self.primary_client.clone(),
        };
        self.websocket_session.auth_owner_generation = self.client.auth_owner_generation();
        self.active_model_route = route;
    }
}

fn deduplicate_additional_tool_declarations(
    input: &mut Vec<ResponseItem>,
    current_tools: &[ToolSpec],
) {
    let current_names = current_tools
        .iter()
        .flat_map(|tool| {
            let mut names = vec![tool.name().to_string()];
            if let ToolSpec::Namespace(namespace) = tool {
                names.extend(namespace.tools.iter().map(|nested| match nested {
                    codex_tools::ResponsesApiNamespaceTool::Function(function) => {
                        function.name.clone()
                    }
                    codex_tools::ResponsesApiNamespaceTool::Custom(custom) => custom.name.clone(),
                }));
            }
            names
        })
        .collect::<std::collections::HashSet<_>>();
    for item in input {
        if let ResponseItem::AdditionalTools { tools, .. } = item {
            let mut deduplicated = tools.to_vec();
            deduplicated.retain_mut(|tool| {
                if tool
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|name| current_names.contains(name))
                {
                    return false;
                }
                if tool.get("type").and_then(serde_json::Value::as_str) == Some("namespace") {
                    let Some(nested) = tool
                        .get_mut("tools")
                        .and_then(serde_json::Value::as_array_mut)
                    else {
                        return true;
                    };
                    nested.retain_mut(|nested| {
                        nested
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .is_none_or(|name| !current_names.contains(name))
                    });
                    return !nested.is_empty();
                }
                true
            });
            *tools = deduplicated.into();
        }
    }
}

fn sanitize_tool_declaration(tool: &mut serde_json::Value) -> bool {
    match tool.get("type").and_then(serde_json::Value::as_str) {
        Some("custom") => {
            tool.get("name").and_then(serde_json::Value::as_str) == Some("apply_patch")
        }
        Some("namespace") => {
            let Some(nested) = tool
                .get_mut("tools")
                .and_then(serde_json::Value::as_array_mut)
            else {
                return true;
            };
            nested.retain_mut(sanitize_tool_declaration);
            !nested.is_empty()
        }
        _ => true,
    }
}

fn rewrite_legacy_custom_tool_history(input: &mut Vec<ResponseItem>) {
    for item in input.iter_mut() {
        if let ResponseItem::AdditionalTools { tools, .. } = item {
            let mut portable = tools.to_vec();
            portable.retain_mut(sanitize_tool_declaration);
            *tools = portable.into();
        }
    }
    let apply_patch_call_ids = input
        .iter()
        .filter_map(|item| match item {
            ResponseItem::CustomToolCall { call_id, name, .. } if name == "apply_patch" => {
                Some(call_id.clone())
            }
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    for item in input {
        let replacement = match item {
            ResponseItem::CustomToolCall {
                id,
                status,
                call_id,
                name,
                input,
                ..
            } if name != "apply_patch" => Some(ResponseItem::Message {
                encrypted_content: None,
                status: status.clone(),
                id: id.clone(),
                role: "assistant".into(),
                content: vec![ContentItem::OutputText {
                    text: format!(
                        "Legacy custom tool call `{name}` with call ID `{call_id}` used this input:\n{input}"
                    ),
                    annotations: None,
                    logprobs: None,
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }),
            ResponseItem::CustomToolCallOutput {
                id,
                call_id,
                name,
                output,
                ..
            } if name.as_deref() != Some("apply_patch")
                && !apply_patch_call_ids.contains(call_id) =>
            {
                let output = output
                    .body
                    .to_text()
                    .unwrap_or_else(|| "<non-text tool output>".into());
                Some(ResponseItem::Message {
                    encrypted_content: None,
                    status: None,
                    id: id.clone(),
                    role: "user".into(),
                    content: vec![ContentItem::InputText {
                        text: format!(
                            "Legacy custom tool output for call ID `{call_id}`:\n{output}"
                        ),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                })
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            *item = replacement;
        }
    }
}

fn reasoning_key(item: &ResponseItem) -> Option<String> {
    use sha2::Digest;
    if let ResponseItem::Reasoning {
        id,
        encrypted_content,
        summary,
        content,
        ..
    } = item
    {
        let value =
            serde_json::to_vec(&(id, encrypted_content, summary, content)).unwrap_or_default();
        Some(format!("{:x}", sha2::Sha256::digest(value)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_tools::ResponsesApiTool;

    #[test]
    fn legacy_custom_exec_becomes_text_while_apply_patch_stays_custom() {
        // Given completed legacy exec and apply_patch history items.
        let mut input = vec![
            ResponseItem::CustomToolCall {
                encrypted_content: None,
                id: None,
                status: Some("completed".into()),
                call_id: "legacy-exec".into(),
                name: "exec".into(),
                namespace: None,
                input: "text(await tools.exec_command({cmd:\"pwd\"}));".into(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CustomToolCallOutput {
                id: None,
                call_id: "legacy-exec".into(),
                name: None,
                output: FunctionCallOutputPayload::from_text("/tmp".into()),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CustomToolCall {
                encrypted_content: None,
                id: None,
                status: Some("completed".into()),
                call_id: "patch-call".into(),
                name: "apply_patch".into(),
                namespace: None,
                input: "*** Begin Patch".into(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CustomToolCallOutput {
                id: None,
                call_id: "patch-call".into(),
                name: None,
                output: FunctionCallOutputPayload::from_text("patched".into()),
                internal_chat_message_metadata_passthrough: None,
            },
        ];

        // When converting history for a different provider.
        rewrite_legacy_custom_tool_history(&mut input);

        // Then exec is portable text and apply_patch retains its native tool shape.
        assert_matches!(
            &input[0],
            ResponseItem::Message { role, content, .. }
            if role == "assistant"
                && content[0].clone().into_text().is_some_and(|text| {
                    text.contains("legacy-exec") && text.contains("pwd")
                })
        );
        assert_matches!(
            &input[1],
            ResponseItem::Message { role, content, .. }
            if role == "user"
                && content[0].clone().into_text().is_some_and(|text| {
                    text.contains("legacy-exec") && text.contains("/tmp")
                })
        );
        assert_matches!(input[2], ResponseItem::CustomToolCall { .. });
        assert_matches!(input[3], ResponseItem::CustomToolCallOutput { .. });
    }

    #[test]
    fn legacy_custom_exec_is_removed_from_additional_tool_declarations() {
        let mut input = vec![ResponseItem::AdditionalTools {
            id: None,
            role: "developer".into(),
            tools: Arc::new([serde_json::json!({
                "type": "namespace",
                "name": "functions",
                "tools": [
                    {"type": "custom", "name": "exec", "description": "legacy"},
                    {"type": "custom", "name": "apply_patch", "format": {"type": "grammar", "syntax": "lark", "definition": "start: /x/"}},
                    {"type": "function", "name": "exec_command", "parameters": {"type": "object"}}
                ]
            })]),
        }];

        rewrite_legacy_custom_tool_history(&mut input);

        let ResponseItem::AdditionalTools { tools, .. } = &input[0] else {
            panic!("expected additional tools");
        };
        let wire = serde_json::to_string(tools).unwrap();
        assert!(!wire.contains("\"name\":\"exec\""));
        assert!(wire.contains("\"name\":\"apply_patch\""));
        assert!(wire.contains("\"name\":\"exec_command\""));
    }

    #[test]
    fn current_tool_names_win_over_legacy_additional_tools() {
        let current_tools = vec![ToolSpec::Function(ResponsesApiTool {
            name: "request_user_input".into(),
            description: "current tool".into(),
            strict: false,
            defer_loading: None,
            parameters: serde_json::from_value(serde_json::json!({"type": "object"})).unwrap(),
            output_schema: None,
        })];
        let mut input = vec![ResponseItem::AdditionalTools {
            id: None,
            role: "developer".into(),
            tools: Arc::new([serde_json::json!({
                "type": "namespace",
                "name": "functions",
                "tools": [
                    {"type": "function", "name": "request_user_input", "parameters": {"type": "object"}},
                    {"type": "custom", "name": "apply_patch", "format": {"type": "grammar", "syntax": "lark", "definition": "start: /x/"}}
                ]
            })]),
        }];

        deduplicate_additional_tool_declarations(&mut input, &current_tools);

        let ResponseItem::AdditionalTools { tools, .. } = &input[0] else {
            panic!("expected additional tools");
        };
        let wire = serde_json::to_string(tools).unwrap();
        assert!(!wire.contains("request_user_input"));
        assert!(wire.contains("apply_patch"));
    }
}
