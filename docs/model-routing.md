# Internal model routing

OpenAI and Kimi share one Codex model selector and one conversation. Selecting a
Kimi model changes the upstream for subsequent turns; work already in progress
continues on its original upstream. There is no separate HTTP proxy process and
no global profile rewrite. The stored thread provider remains the primary
provider, while the selected model determines inference routing.

## Configuration

Keep the primary provider OpenAI and its native catalog. The optional
`$CODEX_HOME/model-routes.json` contains a `routes` array. Each entry contains:

- `provider`: `name`, HTTPS `base_url`, and the credential variable name `env_key`.
- `models`: full Codex `ModelInfo` records, including exact slugs, display names,
  reasoning options, context limits and tool capabilities.

For Kimi use `name: "Kimi"`, `base_url: "https://api.kimi.ai/coding/v1"` and
`env_key: "KIMI_API_KEY"`. Existing full records from a Kimi `models.json` can
populate `models`; the local setup contains `k3.1-pro` and
`k3.1-pro-highspeed`. Credential values remain in the environment. Missing route
files retain normal Codex behavior. Invalid files fail inference initialization
without echoing their contents. Restart the backend after editing route metadata.

Kimi natively supports Responses, including reasoning, streaming and function
calls; see its [Codex guide](https://www.kimi.com/code/docs/third-party-tools/codex.html).
Keep `shell_type: "shell_command"` and `apply_patch_tool_type: "freeform"` so
shell operations use function tools and patches use the supported custom
`apply_patch` tool. A custom `exec` tool is not supported and must not be
substituted for the function-based shell contract. No Chat Completions
translation is involved.

## Isolation and conversation history

Additional models are appended without changing OpenAI's catalog cache or
account filtering. Route matching uses exact model slugs. Each routed client has
independent authentication and transport state. Crossing a route discards
websocket continuation, response IDs and sticky turn state. OpenAI account
routing, runtime identity, attestation, request contributors, compression and
service tiers are not inherited by Kimi. Missing/rejected routed credentials
fail on that route, never fall back to OpenAI.

Visible conversation is replayed across providers. Initial opaque reasoning is
excluded from the next turn, while new reasoning needed by the current
upstream's tool loop is retained. Legacy custom tool calls other than
`apply_patch` become portable text records, so an old OpenAI `exec` item is not
replayed as an unsupported custom tool on Kimi. Historical `AdditionalTools`
are filtered by the same rule and deduplicated against current request tools,
with current declarations taking precedence. Routed sessions use local text
compaction.
Legacy OpenAI encrypted compaction is converted to a portable summary before
Kimi receives it. If the historical OpenAI model is unavailable, a supported
current native OpenAI model retries only that compaction; the following turn
can then use Kimi with portable history. Selecting Kimi before native recovery
fails closed: no opaque OpenAI state is sent to Kimi and no OpenAI user-answer
fallback is attempted.

## Validation

The current regression coverage includes:

- Three configuration tests for exact routes, isolated credentials, invalid
  destinations and sanitized errors, plus one catalog-overlay test.
- Three core history/declaration tests prove that legacy custom `exec` becomes
  portable text, unsupported declarations are removed recursively, and current
  tool names win over historical `AdditionalTools`. `apply_patch` retains its
  native tool shape.
- Sixteen app-server tests for exact routing, OpenAI → Kimi → OpenAI history,
  busy turns in both directions, streamed text and tool output, credential
  isolation, resume, ChatGPT authentication, legacy-compaction recovery,
  fail-closed behavior, redirect pinning, Responses Lite normalization, retry
  exhaustion and native/routed tool round trips.
- Eight model-picker tests for slug/display-name search, session-only Kimi
  selection, busy-turn/draft preservation, 100-column dialog geometry, and the
  single model → effort replacement window. Three view-stack geometry tests
  preserve viewport clamping and the default 72-column cap for other dialogs.
- Four daemon package tests and a workspace Clippy run. Components excluded
  because the external V8 artifact is unavailable are reported separately;
  they are not claimed as passing.

Final live verification used ordinary `codex` attached to the managed daemon,
without `--no-daemon` or provider overrides. Both providers returned real text,
completed safe tool calls, and preserved the active turn when switched in both
directions. Kimi completed both shell and `apply_patch` calls. The original
historical thread first completed native recovery and then completed a Kimi
turn after portable compaction. Model selection remained session-only and did
not rewrite the global profile.

## Local rollout

The installed CLI and managed daemon include the router. Use ordinary `codex`
and select models with `Ctrl+X M` or `/model`; the former `kimi` shell shortcut
is removed. No `--no-daemon` bridge or global profile rewrite is required.

The custom package was installed and pinned with
`codex app-server daemon update --from-cli -y` after confirming that no turns
were active. The ordinary CLI, managed daemon and source-built package were
verified to have matching SHA-256 hashes at rollout. Subsequent route-metadata
edits require a daemon restart. Do not update or restart the daemon against
active turns: the operation can interrupt their backend.
