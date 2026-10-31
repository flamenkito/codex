# Local terminal appearance patch

Base: `openai-oss-forks/crossterm` commit
`ed1cdab335221515706178d68495bba2aed1924f` (MIT; see LICENSE).

The async EventStream additionally exposes OSC 10/11 RGB replies and DEC mode
2031 appearance notifications. Synchronous read/poll and startup color probes
keep their existing filtering behavior. The normal reader continues to own stdin;
there is no competing raw reader during runtime refresh.

Regression tests cover theme notifications, stream filtering, and fragmented
OSC replies interleaved with keys and bracketed paste on both Unix backends.

This directory is selected by the Cargo patch in `codex-rs/Cargo.toml`.
The Bazel dependency definition is intentionally unchanged; use Cargo for this
temporary local build. Track upstream resolution in openai/codex#45163.
