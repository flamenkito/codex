//! Nonblocking runtime color refresh: the normal event reader owns every input byte.
use crate::terminal_probe::DefaultColors;
use std::time::Duration;
use std::time::Instant;

const TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Default)]
pub(super) struct PaletteRefresh {
    started: Option<Instant>,
    fg: Option<(u8, u8, u8)>,
    bg: Option<(u8, u8, u8)>,
}

impl PaletteRefresh {
    pub(super) fn begin(&mut self, now: Instant) -> bool {
        if self
            .started
            .is_some_and(|start| now.duration_since(start) < TIMEOUT)
        {
            return false;
        }
        self.started = Some(now);
        self.fg = None;
        self.bg = None;
        true
    }

    pub(super) fn accept(
        &mut self,
        slot: u8,
        rgb: (u8, u8, u8),
        now: Instant,
    ) -> Option<DefaultColors> {
        let start = self.started?;
        if now.duration_since(start) >= TIMEOUT {
            self.started = None;
            return None;
        }
        match slot {
            10 => self.fg = Some(rgb),
            11 => self.bg = Some(rgb),
            _ => return None,
        }
        let colors = DefaultColors {
            fg: self.fg?,
            bg: self.bg?,
        };
        self.started = None;
        Some(colors)
    }

    /// Replies can arrive inside bracketed paste. Strip only complete, valid color
    /// replies during an outstanding query; preserve every other pasted byte.
    pub(super) fn filter_paste(&mut self, text: String) -> String {
        if self.started.is_none() {
            return text;
        }
        let bytes = text.as_bytes();
        let mut output = Vec::with_capacity(bytes.len());
        let mut cursor = 0;
        while cursor < bytes.len() {
            if bytes[cursor..].starts_with(b"\x1b]")
                && let Some((end, terminator)) =
                    crate::terminal_probe::osc_payload_end(&bytes[cursor + 2..])
            {
                let next = cursor + 2 + end + terminator;
                let reply = &bytes[cursor..next];
                if let Some((slot, rgb)) = [10, 11].into_iter().find_map(|slot| {
                    crate::terminal_probe::parse_osc_color(reply, slot).map(|rgb| (slot, rgb))
                }) {
                    if let Some(colors) = self.accept(slot, rgb, Instant::now()) {
                        crate::terminal_palette::update_default_colors(colors);
                    }
                    cursor = next;
                    continue;
                }
            }
            output.push(bytes[cursor]);
            cursor += 1;
        }
        // Removing ASCII protocol sequences cannot invalidate the original UTF-8.
        String::from_utf8(output).unwrap_or(text)
    }

    pub(super) fn request(&mut self) {
        if !self.begin(Instant::now()) {
            return;
        }
        #[cfg(all(unix, not(test)))]
        {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            if out
                .write_all(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\")
                .and_then(|()| out.flush())
                .is_err()
            {
                self.started = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replies_inside_paste_do_not_become_draft_text() {
        let mut refresh = PaletteRefresh::default();
        refresh.begin(Instant::now());
        assert_eq!(
            refresh.filter_paste(
                "aé\x1b]11;rgb:1111/2222/3333\x07bc\x1b]10;rgb:ffff/ffff/ffff\x1b\\d".into()
            ),
            "aébcd"
        );
        let literal = "keep \x1b]11;rgb:1111/2222/3333\x07".to_string();
        assert_eq!(refresh.filter_paste(literal.clone()), literal);
    }

    #[test]
    fn accepts_reordered_complete_replies_only() {
        let now = Instant::now();
        let mut refresh = PaletteRefresh::default();
        assert!(refresh.begin(now));
        assert!(!refresh.begin(now));
        assert!(refresh.accept(11, (1, 2, 3), now).is_none());
        assert!(refresh.accept(9, (0, 0, 0), now).is_none());
        assert_eq!(
            refresh.accept(10, (4, 5, 6), now),
            Some(DefaultColors {
                fg: (4, 5, 6),
                bg: (1, 2, 3)
            })
        );
        assert!(refresh.accept(10, (0, 0, 0), now).is_none());
    }
    #[test]
    fn timeout_drops_partial_sample_and_allows_retry() {
        let now = Instant::now();
        let mut refresh = PaletteRefresh::default();
        refresh.begin(now);
        refresh.accept(10, (1, 1, 1), now);
        assert!(refresh.accept(11, (2, 2, 2), now + TIMEOUT).is_none());
        assert!(refresh.begin(now + TIMEOUT));
        assert!(refresh.accept(11, (3, 3, 3), now + TIMEOUT).is_none());
        assert_eq!(
            refresh.accept(10, (4, 4, 4), now + TIMEOUT),
            Some(DefaultColors {
                fg: (4, 4, 4),
                bg: (3, 3, 3)
            })
        );
    }
}
