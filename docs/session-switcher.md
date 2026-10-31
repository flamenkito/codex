# Local session-switcher extension

The custom CLI opens a compact **Sessions** panel with **Ctrl+X, then L**.
The implementation is a native Rust module, not a marketplace plugin: current
Codex plugins cannot register terminal widgets, key actions, or thread-switch
handlers. The module lives in `tui/src/resume_picker/switcher.rs`, with small
host adapters for key registration, the existing catalog, and backend actions.
The ordinary `/resume` and startup/fork pickers keep their existing presentation.

The compact panel overlays a frozen snapshot of the current conversation and
composer. Its borderless, theme-aware surface (with an ANSI fallback when the
terminal color probe is unavailable) is capped at 88 columns, places
`esc` at the right edge of the title row, and keeps the three management actions
on one footer line. A second footer row is reserved for status and errors. Its
geometry is derived from the unfiltered catalog, so searching—including a search
with no matches—does not resize the panel. Only the panel rectangle is cleared.
In inline (`--no-alt-screen`)
mode, transcript rows above the composer are reconstructed from replayable
history, since those rows are held by terminal scrollback rather than the frame
buffer. Resizing clips the snapshot safely; closing restores the chat view.

## Controls

| Key | Action |
| --- | --- |
| Type | Search session titles, case-insensitively |
| Up / Down, Page Up / Page Down | Navigate the existing paginated catalog |
| Enter | Switch using the existing draft-preserving transition |
| Esc | Clear the search first, then close without changing the draft |
| Ctrl+F | Pin/unpin the selected session |
| Ctrl+R | Edit its title; Enter saves, Esc cancels |
| Ctrl+D twice | Delete the selected session; switching away from the current session is required first |
| Tab, Left / Right, Ctrl+X then F | Toggle current-directory/all-directory scope |
| Ctrl+X then 1–9, inside the panel | Open a numbered quick-switch slot |

The panel lists top-level conversations, groups pins first and then local dates,
and shows working/waiting/error/current indicators and short directory hints.
Visible activity refreshes every five seconds. Failed status reads show `unknown`.
Quick slots are frozen when the first rows arrive, so search and pagination do
not silently retarget them. A slot whose session is absent does not open another
session. Keyboard navigation still uses the existing list keymap; Ctrl+F is pin
in this panel, so use Page Down for paging.

`tui.keymap.global.open_sessions` controls the opening shortcut. Its implicit
default is `leader l`, with the existing leader defaulting to `ctrl-x`. Explicit
bindings and explicit unbinding take precedence. Customize through `/keymap` or:

```toml
[tui.keymap.global]
open_sessions = "leader l"
```

Pins are saved separately in `$CODEX_HOME/session-switcher.json` (normally
`~/.codex/session-switcher.json`). File locking serializes writers and an atomic
replacement prevents partial JSON. Invalid JSON is reported and preserved.
Preferences contain thread IDs only; thread actions use the row's backend owner.
Mutation completions are also tagged with their backend.

## Build and local installation

Use the pinned toolchain and `dev-small` profile. The task used an isolated Cargo
cache at `/tmp/codex-session-switcher-cargo` because another build held the normal
cache lock. The V8 manifest override documented in `local-theme-refresh.md`
avoids unrelated Chromium submodule downloads.

```sh
cd "/Volumes/WD 4TB/projects/codex/codex-rs"
env CARGO_HOME=/tmp/codex-session-switcher-cargo cargo build --offline \
  --config 'patch.crates-io.v8.path="/Users/asutov/.cargo/git/checkouts/rusty_v8-1c356b58a53d1d98/350c5a8"' \
  -p codex-cli --bin codex --profile dev-small
cd ..
python3 scripts/install_local_session_switcher.py
# Undo this extension while retaining the previous custom build:
python3 scripts/install_local_session_switcher.py --restore
```

The installer checks the existing custom-build record, preserves that binary,
and keeps the earlier restore-to-official command usable. Restart/resume a CLI
session after installation. Package updates can replace custom executables.

## Regression invariants

- Input stays in the normal TUI event stream. Opening/cancelling the picker must
  preserve the composer draft, and switching uses the existing per-thread input
  capture/restore logic.
- Rendering and pagination share the same row-height calculation, including date
  headers. Use `line_truncation::truncate_line_with_ellipsis_if_overflow` when
  fitting terminal columns: `truncate_text` counts graphemes and can overflow
  with wide Unicode characters. The Unicode row test caught this during development.
- Pin sorting preserves the selected row; paging/search do not reassign slots.
- Mutations require an explicit action, delete requires confirmation, and a
  completion cannot update a different backend's copy of a thread.
- The compact panel must not persist density changes to the ordinary picker.
- Every popup frame must paint its backdrop before clearing only the panel bounds.
  Test both larger and smaller terminal sizes: stale cells must not leak through
  inside the popup, and conversation cells outside it must survive.

## Verification (2026-10-31)

- 145 picker-related tests passed, including the OpenCode-style popup rendering
  regression and
  existing remote-directory, pagination, preview, cancellation and draft tests.
- 131 keymap tests passed, including default shortcut registration, explicit
  unbinding and conflict precedence.
- `cargo clippy -p codex-tui --profile dev-small --lib` passed without warnings.
- The actual CLI passed PTY checks in normal and `--no-alt-screen` modes for
  Ctrl+X/L, visible background/composer, title search, resizing while searching,
  and cancelling back to the unchanged draft; no prompts were submitted.
- Source-level rendering checks cover the 88-column cap, right-aligned `esc`,
  borderless themed surface, compact footer, and stable search geometry.
- Regenerated `core/config.schema.json` for `open_sessions`.

Use `RUST_MIN_STACK=16777216` for the picker test filter; the app fixture futures
need enlarged test-thread stacks. The first direct run without the repository's
larger-stack test setup overflowed. Mock-server integration tests also require
permission to bind loopback sockets.
