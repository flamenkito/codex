# Temporary local terminal theme refresh

Tracks [openai/codex#45163](https://github.com/openai/codex/issues/45163).
Local source baseline: `55c099a`; original installed package: `0.166.0`.

## Behavior

The TUI requests OSC 10/11 colors on focus regain, DEC mode 2031 appearance
notifications, and every five seconds while focused. The regular crossterm
reader delivers replies; runtime refresh never pauses that reader or starts a
competing stdin reader. Periodic queries stop during terminal handoff.

Only a complete foreground/background sample received within 750 ms replaces
the palette. Missing, malformed, or partial replies retain the last good palette.
Unchanged replies do not invalidate rendering. Changed samples refresh the auto
syntax theme (explicit selections stay selected), invalidate rendering caches,
and replay inline history. Draft content remains intact. Complete color replies
interleaved inside an outstanding bracketed paste are removed from the paste.

The local crossterm copy and its provenance are documented in
[`vendor/crossterm/LOCAL-PATCH.md`](../vendor/crossterm/LOCAL-PATCH.md).

## Build and checks

Use the pinned Rust toolchain from `codex-rs/rust-toolchain.toml`. The local
executable uses the existing `dev-small` profile. Source builds report version
`0.0.0`; the original packaged executable remains backed up.

The first dependency fetch tried to recursively download V8's Chromium
submodules, although the CLI does not build V8. For this workstation only, an
already downloaded checkout supplied V8's manifest through this Cargo option:

```sh
cd /Users/asutov/projects/codex/codex-rs
cargo build --config 'patch.crates-io.v8.path="/Users/asutov/.cargo/git/checkouts/rusty_v8-1c356b58a53d1d98/350c5a8"' -p codex-cli --bin codex --profile dev-small
```

Use that same `--config` option **after** `test` or `clippy` when repeating the
checks. It is a build invocation override, not a change to the project's pinned
V8 dependency. Cargo temporarily removes V8's source line from Cargo.lock when
using this override; retain the original pinned V8 source in the final lockfile.

Verified on 2026-10-31:

- `cargo check -p codex-tui --lib`; `cargo clippy -p codex-tui --lib` (clean).
- 42 palette-related TUI tests; 11 input-stream tests.
- Vendored crossterm library: 145 passed, 7 ignored; alternate Unix backend
  fragmented-input regression also passed.
- Executable PTY test: no focus events, dark → light → dark; composer RGB fill
  changed `(48,48,48)` → `(244,244,244)` → `(48,48,48)` and draft text remained.
  Set `FORCE_COLOR=3` and unset inherited `NO_COLOR` for this rendering test.

The invariant to preserve in future changes is one owner of terminal input:
refresh responses and user input must share framing and event delivery. Restoring
a synchronous focus-time raw reader would regress input preservation.

## Install and undo

The installer targets this Mac's Bun-managed `0.166.0` package. It backs up the
original binary and atomically replaces only `bin/codex`; companion binaries
and resources stay in their installed package. Existing processes keep running
their original executable. Restart/resume a CLI session to use the fix.

```sh
cd /Users/asutov/projects/codex
python3 scripts/install_local_theme_fix.py
# Restore the original executable:
python3 scripts/install_local_theme_fix.py --restore
```

Restore validates both binary hashes before replacing anything. Package updates
may replace the local patch; revalidate against the new release before applying
it again. No issue comment or upstream change was published by this task.
