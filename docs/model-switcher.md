# Local model-switcher extension

**Ctrl+X, then M** opens a searchable model selector over Codex. The shortcut
uses the current account's model catalog and lists all available models,
including auto modes, in one searchable menu. It highlights the current model
and retains the existing reasoning and session-only choices.
Reasoning submenus retain the centered presentation. The ordinary `/model`
command keeps its existing inline presentation.

Type to search, use arrows and Enter to choose a model/reasoning level, or Esc
to go back. **Ctrl+S** applies a session-only choice wherever that action is
available; printable `s` stays available for searching.

The shortcut is configurable through `/keymap` or:

```toml
[tui.keymap.global]
open_models = "leader m"
```

The shared leader defaults to `ctrl-x`. Explicit bindings, explicit unbinding,
and another action already assigned to `leader m` take precedence over the
implicit shortcut. The session selector remains on `leader l`.

## Switching during work

Both session and model shortcuts are available while a turn is running; there
is no need to wait for it to finish or clear the composer. Opening, searching,
and cancelling either menu must not submit a prompt or interrupt the turn.
Model selection uses the existing thread-settings update path; it does not
cancel or replace an already-running model request. Session navigation uses the
existing unsubscribe/resume path, which leaves server work running. Returning
to the session restores its updated state and its retained input.

The model overlay runs in the main app event loop, allowing backend events and
model-catalog refreshes to continue. Refreshes must keep the overlay presentation
and highlighted wire model ID. Selection semantics and permission/profile checks
remain those of the normal model picker.

## Verification

Focused regressions cover implicit and customized shortcuts, routing both
shortcuts during a running turn, searchable model overlays, catalog refresh,
reasoning submenus, cancellation, draft retention, and absence of interrupt
operations. Backend tests separately cover settings updates and unsubscribe
while a turn is running.

Searchable menus distinguish an open view from a selected result: a zero-match
query must survive catalog refresh without reopening or duplicating the menu.

Verified on 2026-10-31:

- 210 keymap-related tests, 112 model-related tests, and 14 switcher tests passed.
- The existing backend active-turn settings-update regression passed.
- All five existing active-turn unsubscribe cases passed, including zero-delay
  unloading and persistent-session cold resume. These two backend modules were
  run through a temporary focused harness; the harness was removed afterwards.
- Real CLI PTY checks passed in normal and `--no-alt-screen` modes for opening,
  centered placement, visible background, search, resizing, and draft retention.
  No prompt was submitted and no real model selection was applied.

PTY checks must wait for the initialized model status line: the initial startup
composer appears before global shortcuts are ready. The model overlay uses the
shared native centered-dialog style, which is borderless.
