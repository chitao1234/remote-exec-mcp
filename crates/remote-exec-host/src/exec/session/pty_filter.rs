use std::borrow::Cow;

use vte::{Params, Parser, Perform};

pub(super) struct TerminalOutputState {
    parser: Parser,
}

#[derive(Debug, Default)]
pub(super) struct TerminalOutputResult {
    pub(super) output: String,
    pub(super) response: String,
}

impl Default for TerminalOutputState {
    fn default() -> Self {
        Self {
            parser: Parser::new(),
        }
    }
}

impl TerminalOutputState {
    pub(super) fn filter_chunk(&mut self, chunk: &str) -> TerminalOutputResult {
        let mut performer = TerminalOutputPerformer::default();
        self.parser.advance(&mut performer, chunk.as_bytes());

        TerminalOutputResult {
            output: performer.output,
            response: performer.response,
        }
    }

    pub(super) fn drain_pending(&mut self) -> String {
        String::new()
    }
}

#[derive(Debug, Default)]
struct TerminalOutputPerformer {
    output: String,
    response: String,
}

impl Perform for TerminalOutputPerformer {
    fn print(&mut self, ch: char) {
        self.output.push(ch);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => self.output.push('\r'),
            b'\n' => self.output.push('\n'),
            b'\t' => self.output.push('\t'),
            0x08 => {
                self.output.pop();
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], ignore: bool, action: char) {
        if ignore || action != 'n' {
            return;
        }

        match first_csi_param(params) {
            Some(5) => self.response.push_str("\x1b[0n"),
            Some(6) => self.response.push_str("\x1b[1;1R"),
            _ => {}
        }
    }
}

fn first_csi_param(params: &Params) -> Option<u16> {
    params
        .iter()
        .next()
        .and_then(|param| param.iter().copied().next())
}

pub(super) fn normalize_input(chars: &str, tty: bool) -> Cow<'_, str> {
    if !tty || !chars.contains('\n') {
        return Cow::Borrowed(chars);
    }

    let mut normalized = String::with_capacity(chars.len());
    let mut last_was_cr = false;

    for ch in chars.chars() {
        match ch {
            '\r' => {
                normalized.push('\r');
                last_was_cr = true;
            }
            '\n' => {
                if !last_was_cr {
                    normalized.push('\r');
                }
                last_was_cr = false;
            }
            _ => {
                normalized.push(ch);
                last_was_cr = false;
            }
        }
    }

    Cow::Owned(normalized)
}

#[cfg(test)]
mod tests {
    use super::{TerminalOutputState, normalize_input};

    #[test]
    fn terminal_output_state_replies_to_device_status_report() {
        let mut state = TerminalOutputState::default();
        let result = state.filter_chunk("before\x1b[5nafter");

        assert_eq!(result.output, "beforeafter");
        assert_eq!(result.response, "\x1b[0n");
        assert_eq!(state.drain_pending(), "");
    }

    #[test]
    fn terminal_output_state_replies_to_cursor_position_report() {
        let mut state = TerminalOutputState::default();
        let result = state.filter_chunk("before\x1b[6nafter");

        assert_eq!(result.output, "beforeafter");
        assert_eq!(result.response, "\x1b[1;1R");
        assert_eq!(state.drain_pending(), "");
    }

    #[test]
    fn terminal_output_state_handles_split_query_sequences() {
        let mut state = TerminalOutputState::default();

        let first = state.filter_chunk("before\x1b[");
        assert_eq!(first.output, "before");
        assert_eq!(first.response, "");

        let second = state.filter_chunk("6nafter");
        assert_eq!(second.output, "after");
        assert_eq!(second.response, "\x1b[1;1R");
        assert_eq!(state.drain_pending(), "");
    }

    #[test]
    fn terminal_output_state_strips_conpty_control_sequences() {
        let mut state = TerminalOutputState::default();
        let result = state
            .filter_chunk("\x1b[m\x1b]0;C:\\Windows\\system32\\cmd.exe\x07\x1b[?25hhello \r\n");

        assert_eq!(result.output, "hello \r\n");
        assert_eq!(result.response, "");
        assert_eq!(state.drain_pending(), "");
    }

    #[test]
    fn terminal_output_state_strips_winpty_control_sequences() {
        let mut state = TerminalOutputState::default();
        let result = state.filter_chunk("\x1b[0m\x1b[0Khello\x1b[0K\x1b[?25l\r\n\x1b[0K\x1b[?25h");

        assert_eq!(result.output, "hello\r\n");
        assert_eq!(result.response, "");
        assert_eq!(state.drain_pending(), "");
    }

    #[test]
    fn tty_input_normalization_converts_bare_lf_to_cr() {
        assert_eq!(normalize_input("ping\n", true).as_ref(), "ping\r");
    }

    #[test]
    fn tty_input_normalization_coalesces_crlf_to_cr() {
        assert_eq!(
            normalize_input("ping\r\npong\n", true).as_ref(),
            "ping\rpong\r"
        );
    }

    #[test]
    fn tty_input_normalization_leaves_existing_cr_unchanged() {
        assert_eq!(normalize_input("ping\r", true).as_ref(), "ping\r");
    }
}
