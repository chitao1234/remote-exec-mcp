use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Instant;

use super::{TerminalOutputFilter, TerminalOutputResult};

const WINPTY_TRANSCRIPT_DEBOUNCE_MS: u64 = 150;
const WINPTY_TRANSCRIPT_MAX_HOLD_MS: u64 = 500;

pub(super) struct WinptyOutputState {
    renderer: TerminalOutputRenderer,
}

impl Default for WinptyOutputState {
    fn default() -> Self {
        Self {
            renderer: TerminalOutputRenderer::new(
                WINPTY_TRANSCRIPT_DEBOUNCE_MS,
                WINPTY_TRANSCRIPT_MAX_HOLD_MS,
            ),
        }
    }
}

impl TerminalOutputFilter for WinptyOutputState {
    fn filter_chunk(&mut self, chunk: &str) -> TerminalOutputResult {
        self.renderer.filter_chunk(chunk)
    }

    fn flush_due(&mut self) -> String {
        self.renderer.flush_due_at(monotonic_ms())
    }

    fn drain_pending(&mut self) -> String {
        self.renderer.drain_pending()
    }
}

#[cfg(test)]
impl WinptyOutputState {
    fn filter_chunk_at(&mut self, chunk: &str, now_ms: u64) -> TerminalOutputResult {
        self.renderer.filter_chunk_at(chunk, now_ms)
    }

    fn flush_due_at(&mut self, now_ms: u64) -> String {
        self.renderer.flush_due_at(now_ms)
    }
}

#[derive(Debug, Clone, Default)]
struct Cell {
    text: String,
    continuation: bool,
    explicit_space: bool,
    has_text: bool,
    width: usize,
}

#[derive(Debug, Default)]
struct Line {
    cells: Vec<Cell>,
    touched: bool,
}

#[derive(Debug, Default)]
struct PendingRow {
    first_pending_at_ms: Option<u64>,
    last_changed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RendererState {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    OscString,
    IgnoreString,
}

#[derive(Debug)]
struct TerminalOutputRenderer {
    lines: Vec<Line>,
    touched_rows: Vec<usize>,
    pending_rows: BTreeMap<usize, PendingRow>,
    closed_row_text: BTreeMap<usize, String>,
    closed_row_joinable: BTreeMap<usize, bool>,
    state: RendererState,
    csi_buffer: String,
    current_row: i32,
    current_col: i32,
    has_open_row: bool,
    open_row: usize,
    open_row_text: String,
    debounce_ms: u64,
    max_hold_ms: u64,
}

impl Default for TerminalOutputRenderer {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl TerminalOutputRenderer {
    fn new(debounce_ms: u64, max_hold_ms: u64) -> Self {
        Self {
            lines: Vec::new(),
            touched_rows: Vec::new(),
            pending_rows: BTreeMap::new(),
            closed_row_text: BTreeMap::new(),
            closed_row_joinable: BTreeMap::new(),
            state: RendererState::Ground,
            csi_buffer: String::new(),
            current_row: 0,
            current_col: 0,
            has_open_row: false,
            open_row: 0,
            open_row_text: String::new(),
            debounce_ms,
            max_hold_ms,
        }
    }

    pub(super) fn filter_chunk(&mut self, chunk: &str) -> TerminalOutputResult {
        self.filter_chunk_at(chunk, monotonic_ms())
    }

    fn filter_chunk_at(&mut self, chunk: &str, now_ms: u64) -> TerminalOutputResult {
        let mut response = String::new();
        for ch in chunk.chars() {
            self.process_char(ch, &mut response);
        }

        let output = if self.debounce_enabled() {
            self.queue_touched_rows(now_ms);
            self.emit_due_rows(now_ms)
        } else {
            self.emit_touched_rows()
        };

        TerminalOutputResult { output, response }
    }

    fn flush_due_at(&mut self, now_ms: u64) -> String {
        if self.debounce_enabled() {
            self.queue_touched_rows(now_ms);
            self.emit_due_rows(now_ms)
        } else {
            self.emit_touched_rows()
        }
    }

    pub(super) fn drain_pending(&mut self) -> String {
        self.state = RendererState::Ground;
        self.csi_buffer.clear();

        let output = if self.debounce_enabled() {
            self.emit_all_pending_rows()
        } else {
            self.emit_touched_rows()
        };
        self.has_open_row = false;
        self.open_row_text.clear();

        output
    }

    fn debounce_enabled(&self) -> bool {
        self.debounce_ms > 0
    }

    fn process_char(&mut self, ch: char, response: &mut String) {
        match self.state {
            RendererState::Ground => {
                if ch == '\x1b' {
                    self.state = RendererState::Escape;
                } else if is_c0_control(ch) {
                    self.emit_control(ch);
                } else {
                    self.emit_printable_char(ch);
                }
            }
            RendererState::Escape => {
                if ch == '[' {
                    self.state = RendererState::Csi;
                    self.csi_buffer.clear();
                } else if ch == ']' {
                    self.state = RendererState::OscString;
                } else if matches!(ch, 'P' | 'X' | '^' | '_') {
                    self.state = RendererState::IgnoreString;
                } else if (' '..='/').contains(&ch) {
                    self.state = RendererState::EscapeIntermediate;
                } else if ch != '\x1b' {
                    self.state = RendererState::Ground;
                }
            }
            RendererState::EscapeIntermediate => {
                if ch == '\x1b' {
                    self.state = RendererState::Escape;
                } else if ('0'..='~').contains(&ch) {
                    self.state = RendererState::Ground;
                }
            }
            RendererState::Csi => {
                if ch == '\x1b' {
                    self.state = RendererState::Escape;
                    self.csi_buffer.clear();
                } else if ('@'..='~').contains(&ch) {
                    self.dispatch_csi(ch, response);
                    self.state = RendererState::Ground;
                    self.csi_buffer.clear();
                } else if ('0'..='?').contains(&ch) || ch == ';' {
                    self.csi_buffer.push(ch);
                }
            }
            RendererState::OscString => {
                if ch == '\x07' {
                    self.state = RendererState::Ground;
                } else if ch == '\x1b' {
                    self.state = RendererState::IgnoreString;
                }
            }
            RendererState::IgnoreString => {
                if ch == '\\' || ch == '\x07' {
                    self.state = RendererState::Ground;
                }
            }
        }
    }

    fn emit_control(&mut self, ch: char) {
        match ch {
            '\r' => {
                self.current_col = 0;
            }
            '\n' => {
                if self.has_open_row && self.open_row == self.normalized_current_row() {
                    self.open_row_text.clear();
                }

                let row = self.normalized_current_row();
                let row_text = self.serialize_physical_line(row);
                let width = self.ensure_line(row).cells.len();
                self.closed_row_joinable
                    .insert(row, closed_row_should_be_joinable(&row_text, width));
                self.closed_row_text.insert(row, row_text);
                self.current_row += 1;
                self.current_col = 0;
            }
            '\t' => loop {
                self.emit_explicit_space();
                if self.current_col % 8 == 0 {
                    break;
                }
            },
            '\x08' => {
                if self.current_col > 0 {
                    self.current_col -= 1;
                }
            }
            _ => {}
        }
    }

    fn emit_printable_char(&mut self, ch: char) {
        if ch == ' ' {
            self.emit_explicit_space();
            return;
        }

        self.write_cell_text(
            ch.to_string(),
            display_width_for_codepoint(ch as u32),
            false,
        );
    }

    fn emit_explicit_space(&mut self) {
        self.write_cell_text(" ".to_string(), 1, true);
    }

    fn write_cell_text(&mut self, text: String, width: usize, explicit_space: bool) {
        if self.current_row < 0 {
            self.current_row = 0;
        }
        if self.current_col < 0 {
            self.current_col = 0;
        }

        let row = self.normalized_current_row();
        let current_col = self.current_col as usize;
        let effective_width = width.max(1);
        let line = self.ensure_line(row);
        clear_cells_range(line, current_col, current_col + effective_width);

        if line.cells.len() < current_col + effective_width {
            line.cells
                .resize(current_col + effective_width, Cell::default());
        }

        let cell = &mut line.cells[current_col];
        cell.text = text;
        cell.continuation = false;
        cell.explicit_space = explicit_space;
        cell.has_text = true;
        cell.width = effective_width;

        for offset in 1..effective_width {
            let trailing = &mut line.cells[current_col + offset];
            trailing.text.clear();
            trailing.continuation = true;
            trailing.explicit_space = false;
            trailing.has_text = false;
            trailing.width = 0;
        }

        self.touch_row(row);
        self.current_col += effective_width as i32;
    }

    fn dispatch_csi(&mut self, action: char, response: &mut String) {
        let mut params = self.csi_buffer.as_str();
        let private_mode = params.strip_prefix('?').is_some();
        if private_mode {
            params = &params[1..];
        }

        let parsed = parse_csi_params(params);
        let first = parsed
            .first()
            .copied()
            .filter(|value| *value != 0)
            .unwrap_or(1);

        match action {
            'A' => {
                self.current_row = (self.current_row - first as i32).max(0);
            }
            'B' => {
                self.current_row += first as i32;
            }
            'C' => {
                self.current_col += first as i32;
            }
            'D' => {
                self.current_col = (self.current_col - first as i32).max(0);
            }
            'G' => {
                self.set_cursor_col(if first == 0 { 0 } else { first as i32 - 1 });
            }
            'H' | 'f' => {
                let row = parsed
                    .first()
                    .copied()
                    .filter(|value| *value != 0)
                    .unwrap_or(1)
                    - 1;
                let col = parsed
                    .get(1)
                    .copied()
                    .filter(|value| *value != 0)
                    .unwrap_or(1)
                    - 1;
                self.current_row = row as i32;
                self.set_cursor_col(col as i32);
            }
            'J' => {
                self.clear_screen_from_cursor(parsed.first().copied().unwrap_or(0) as i32);
            }
            'K' => {
                self.clear_line_from_cursor(parsed.first().copied().unwrap_or(0) as i32);
            }
            'm' => {}
            'h' | 'l' => {
                let _ = private_mode;
            }
            'n' => match parsed.first().copied() {
                Some(5) => response.push_str("\x1b[0n"),
                Some(6) => response.push_str("\x1b[1;1R"),
                _ => {}
            },
            _ => {}
        }
    }

    fn set_cursor_col(&mut self, col: i32) {
        self.current_col = col.max(0);
    }

    fn clear_line_from_cursor(&mut self, mode: i32) {
        if self.current_row < 0 {
            self.current_row = 0;
        }

        let row = self.normalized_current_row();
        let current_col = self.current_col.max(0) as usize;
        {
            let line = self.ensure_line(row);
            if mode == 2 {
                line.cells.clear();
            } else if mode == 1 {
                clear_cells_range(line, 0, current_col + 1);
            } else {
                let end = line.cells.len();
                clear_cells_range(line, current_col, end);
            }
        }
        self.touch_row(row);
    }

    fn clear_screen_from_cursor(&mut self, mode: i32) {
        if mode == 2 {
            self.lines.clear();
            self.touched_rows.clear();
            self.pending_rows.clear();
            self.closed_row_text.clear();
            self.closed_row_joinable.clear();
            self.current_row = 0;
            self.current_col = 0;
            self.has_open_row = false;
            self.open_row_text.clear();
            return;
        }

        if self.current_row < 0 {
            self.current_row = 0;
        }
        self.clear_line_from_cursor(if mode == 1 { 1 } else { 0 });
        if mode == 0 {
            let start = self.current_row.saturating_add(1) as usize;
            for row in start..self.lines.len() {
                self.lines[row].cells.clear();
                self.touch_row(row);
            }
        } else if mode == 1 {
            for row in 0..self.normalized_current_row() {
                self.lines[row].cells.clear();
                self.touch_row(row);
            }
        }
    }

    fn touch_row(&mut self, row: usize) {
        if row >= self.lines.len() {
            self.lines.resize_with(row + 1, Line::default);
        }
        if !self.lines[row].touched {
            self.lines[row].touched = true;
            self.touched_rows.push(row);
        }
    }

    fn ensure_line(&mut self, row: usize) -> &mut Line {
        if row >= self.lines.len() {
            self.lines.resize_with(row + 1, Line::default);
        }
        &mut self.lines[row]
    }

    fn line_has_any_content(&self, row: usize) -> bool {
        self.lines.get(row).is_some_and(line_has_any_content)
    }

    fn serialize_physical_line(&self, row: usize) -> String {
        let Some(line) = self.lines.get(row) else {
            return String::new();
        };

        let mut out = String::new();
        for cell in &line.cells {
            if cell.continuation {
                continue;
            }
            if cell.has_text {
                out.push_str(&cell.text);
            } else if cell.explicit_space {
                out.push(' ');
            }
        }

        while out.ends_with(' ') {
            out.pop();
        }
        out
    }

    fn queue_touched_rows(&mut self, now_ms: u64) {
        if self.touched_rows.is_empty() {
            return;
        }

        self.touched_rows.sort_unstable();
        self.touched_rows.dedup();

        for row in self.touched_rows.drain(..) {
            if row >= self.lines.len() {
                continue;
            }

            let pending = self.pending_rows.entry(row).or_default();
            pending.first_pending_at_ms.get_or_insert(now_ms);
            pending.last_changed_at_ms = Some(now_ms);
            self.lines[row].touched = false;
        }
    }

    fn pending_row_due(&self, pending: &PendingRow, now_ms: u64) -> bool {
        if !self.debounce_enabled() {
            return true;
        }

        let Some(first_pending_at_ms) = pending.first_pending_at_ms else {
            return true;
        };
        let Some(last_changed_at_ms) = pending.last_changed_at_ms else {
            return true;
        };

        now_ms.saturating_sub(last_changed_at_ms) >= self.debounce_ms
            || (self.max_hold_ms > 0
                && now_ms.saturating_sub(first_pending_at_ms) >= self.max_hold_ms)
    }

    fn emit_rows(&mut self, rows: &[usize]) -> String {
        let mut output = String::new();

        for &row in rows {
            if row >= self.lines.len() {
                continue;
            }

            let current = self.serialize_physical_line(row);
            let has_content = self.line_has_any_content(row);

            if !has_content && self.has_open_row && self.open_row == row {
                output.push('\n');
                self.has_open_row = false;
                self.open_row_text.clear();
                self.closed_row_text.remove(&row);
                self.closed_row_joinable.remove(&row);
                self.lines[row].touched = false;
                continue;
            }

            if !has_content && !self.has_open_row {
                self.lines[row].touched = false;
                continue;
            }

            let previous_row = row.saturating_sub(1);
            let previous_closed = self.closed_row_text.get(&previous_row).cloned();
            let closed = self.closed_row_text.get(&row).cloned();
            let closed_joinable =
                closed.is_some() && self.closed_row_joinable.get(&row).copied().unwrap_or(false);
            let join_previous_closed_row = row > 0
                && previous_closed.is_some()
                && self
                    .closed_row_joinable
                    .get(&previous_row)
                    .copied()
                    .unwrap_or(false)
                && line_has_nonspace_char(&current);

            if join_previous_closed_row {
                let merged = merge_joinable_rows(
                    previous_closed
                        .as_ref()
                        .expect("checked previous closed row above"),
                    &current,
                );
                if self.has_open_row {
                    output.push('\n');
                    self.has_open_row = false;
                    self.open_row_text.clear();
                }
                output.push_str(&merged);
                self.has_open_row = true;
                self.open_row = previous_row;
                self.open_row_text = merged.clone();
                self.closed_row_text.remove(&previous_row);
                self.closed_row_joinable.remove(&previous_row);
                if closed.is_some() {
                    self.closed_row_text.insert(row, merged);
                    self.closed_row_joinable.insert(row, closed_joinable);
                }
                self.lines[row].touched = false;
                continue;
            }

            if let Some(closed_text) = closed.as_deref() {
                if !closed_joinable && closed_text == current {
                    if self.has_open_row && self.open_row != row {
                        output.push('\n');
                        self.has_open_row = false;
                        self.open_row_text.clear();
                    }
                    output.push_str(&current);
                    output.push('\n');
                    self.has_open_row = false;
                    self.open_row_text.clear();
                    self.closed_row_text.remove(&row);
                    self.closed_row_joinable.remove(&row);
                    self.lines[row].touched = false;
                    continue;
                }

                if closed_joinable && closed_text == current {
                    self.lines[row].touched = false;
                    continue;
                }

                if closed_text != current && line_has_nonspace_char(&current) {
                    if self.has_open_row {
                        output.push('\n');
                        self.has_open_row = false;
                        self.open_row_text.clear();
                    }
                    output.push_str(&current);
                    output.push('\n');
                    self.closed_row_text.insert(row, current);
                    self.closed_row_joinable.insert(row, false);
                    self.lines[row].touched = false;
                    continue;
                }
            }

            if !line_has_nonspace_char(&current) {
                self.lines[row].touched = false;
                continue;
            }

            if self.has_open_row && self.open_row == row {
                if current != self.open_row_text {
                    let suffix = suffix_after_byte_len(
                        &current,
                        self.open_row_text.len().min(current.len()),
                    );
                    if !suffix.is_empty() {
                        output.push_str(suffix);
                    }
                    self.open_row_text = current;
                }
            } else {
                if self.has_open_row {
                    output.push('\n');
                }
                output.push_str(&current);
                self.has_open_row = true;
                self.open_row = row;
                self.open_row_text = current;
            }

            self.lines[row].touched = false;
        }

        output
    }

    fn emit_touched_rows(&mut self) -> String {
        if self.touched_rows.is_empty() {
            return String::new();
        }

        self.touched_rows.sort_unstable();
        self.touched_rows.dedup();
        let rows = std::mem::take(&mut self.touched_rows);
        self.emit_rows(&rows)
    }

    fn emit_due_rows(&mut self, now_ms: u64) -> String {
        if self.pending_rows.is_empty() {
            return String::new();
        }

        let due_rows = self
            .pending_rows
            .iter()
            .filter_map(|(&row, pending)| self.pending_row_due(pending, now_ms).then_some(row))
            .collect::<Vec<_>>();
        if due_rows.is_empty() {
            return String::new();
        }

        let output = self.emit_rows(&due_rows);
        for row in due_rows {
            self.pending_rows.remove(&row);
        }
        output
    }

    fn emit_all_pending_rows(&mut self) -> String {
        if !self.touched_rows.is_empty() {
            self.queue_touched_rows(monotonic_ms());
        }
        if self.pending_rows.is_empty() {
            return String::new();
        }

        let rows = self.pending_rows.keys().copied().collect::<Vec<_>>();
        let output = self.emit_rows(&rows);
        self.pending_rows.clear();
        output
    }

    fn normalized_current_row(&self) -> usize {
        self.current_row.max(0) as usize
    }
}

fn monotonic_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn is_c0_control(ch: char) -> bool {
    matches!(ch, '\0'..='\x1f' | '\x7f')
}

fn parse_csi_params(raw: &str) -> Vec<usize> {
    if raw.is_empty() {
        return Vec::new();
    }

    raw.split(';')
        .map(|token| {
            let digits = token
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if digits.is_empty() {
                0
            } else {
                digits.parse().unwrap_or(0)
            }
        })
        .collect()
}

fn is_combining_codepoint(codepoint: u32) -> bool {
    (0x0300..=0x036f).contains(&codepoint)
        || (0x1ab0..=0x1aff).contains(&codepoint)
        || (0x1dc0..=0x1dff).contains(&codepoint)
        || (0x20d0..=0x20ff).contains(&codepoint)
        || (0xfe20..=0xfe2f).contains(&codepoint)
}

fn is_wide_codepoint(codepoint: u32) -> bool {
    if codepoint >= 0x1100
        && (codepoint <= 0x115f
            || codepoint == 0x2329
            || codepoint == 0x232a
            || ((0x2e80..=0xa4cf).contains(&codepoint) && codepoint != 0x303f)
            || (0xac00..=0xd7a3).contains(&codepoint)
            || (0xf900..=0xfaff).contains(&codepoint)
            || (0xfe10..=0xfe19).contains(&codepoint)
            || (0xfe30..=0xfe6f).contains(&codepoint)
            || (0xff00..=0xff60).contains(&codepoint)
            || (0xffe0..=0xffe6).contains(&codepoint))
    {
        return true;
    }

    (0x1f300..=0x1faff).contains(&codepoint)
}

fn display_width_for_codepoint(codepoint: u32) -> usize {
    if codepoint == 0 {
        1
    } else if is_combining_codepoint(codepoint) {
        0
    } else if is_wide_codepoint(codepoint) {
        2
    } else {
        1
    }
}

fn line_has_nonspace_char(text: &str) -> bool {
    text.bytes().any(|byte| byte != b' ')
}

fn is_prompt_marker_char(ch: char) -> bool {
    matches!(ch, '>' | '$' | '#')
}

fn closed_row_should_be_joinable(text: &str, physical_width: usize) -> bool {
    physical_width >= 80
        && line_has_nonspace_char(text)
        && !text.bytes().any(|byte| matches!(byte, b' ' | b'\t'))
}

fn merge_joinable_rows(previous: &str, current: &str) -> String {
    if previous.is_empty() {
        return current.to_string();
    }
    if current.is_empty() {
        return previous.to_string();
    }
    if current.starts_with(previous) {
        return current.to_string();
    }
    if let Some(repeated) = current.find(previous) {
        if repeated > 0
            && current[..repeated]
                .chars()
                .last()
                .is_some_and(is_prompt_marker_char)
        {
            return current[repeated..].to_string();
        }
    }

    let previous_chars = previous.chars().collect::<Vec<_>>();
    let current_chars = current.chars().collect::<Vec<_>>();
    let max_overlap = previous_chars.len().min(current_chars.len());
    for overlap in (1..=max_overlap).rev() {
        if previous_chars[previous_chars.len() - overlap..] == current_chars[..overlap] {
            let suffix = current_chars[overlap..].iter().collect::<String>();
            return format!("{previous}{suffix}");
        }
    }

    format!("{previous}{current}")
}

fn line_has_any_content(line: &Line) -> bool {
    line.cells.iter().any(|cell| {
        if cell.continuation {
            false
        } else {
            cell.has_text || cell.explicit_space
        }
    })
}

fn clear_cells_range(line: &mut Line, start_col: usize, end_col: usize) {
    if end_col <= start_col {
        return;
    }

    let mut clear_start = start_col;
    let mut clear_end = end_col;
    if line.cells.len() < clear_end {
        line.cells.resize(clear_end, Cell::default());
    }

    while clear_start > 0 && clear_start < line.cells.len() && line.cells[clear_start].continuation
    {
        clear_start -= 1;
    }

    loop {
        let mut expanded = false;
        if line.cells.len() < clear_end {
            line.cells.resize(clear_end, Cell::default());
        }

        let scan_end = clear_end.min(line.cells.len());
        let mut col = clear_start;
        while col < scan_end {
            let cell = &line.cells[col];
            if cell.continuation {
                let mut lead = col;
                while lead > 0 && line.cells[lead].continuation {
                    lead -= 1;
                }
                if lead < clear_start {
                    clear_start = lead;
                    expanded = true;
                }
                let lead_cell = &line.cells[lead];
                let cell_end = lead + lead_cell.width.max(1);
                if cell_end > clear_end {
                    clear_end = cell_end;
                    expanded = true;
                }
            } else if cell.has_text && cell.width > 1 {
                let cell_end = col + cell.width;
                if cell_end > clear_end {
                    clear_end = cell_end;
                    expanded = true;
                }
            }
            col += 1;
        }

        if !expanded {
            break;
        }
    }

    if line.cells.len() < clear_end {
        line.cells.resize(clear_end, Cell::default());
    }
    for col in clear_start..clear_end {
        line.cells[col] = Cell::default();
    }
}

fn suffix_after_byte_len(text: &str, len: usize) -> &str {
    let start = if text.is_char_boundary(len) {
        len
    } else {
        text.char_indices()
            .map(|(index, _)| index)
            .find(|index| *index >= len)
            .unwrap_or(text.len())
    };
    &text[start..]
}

#[cfg(test)]
mod tests {
    use crate::exec::session::pty_filter::TerminalOutputFilter;

    use super::{TerminalOutputRenderer, WinptyOutputState};

    #[test]
    fn winpty_output_state_strips_control_sequences() {
        let mut state = WinptyOutputState::default();
        let result = state.filter_chunk("\x1b[0m\x1b[0Khello\x1b[0K\x1b[?25l\r\n\x1b[0K\x1b[?25h");

        assert_eq!(result.output, "");
        assert_eq!(result.response, "");
        assert_eq!(state.drain_pending(), "hello\n");
    }

    #[test]
    fn winpty_terminal_output_state_replies_to_device_status_report() {
        let mut state = WinptyOutputState::default();
        let result = state.filter_chunk("before\x1b[5nafter");

        assert_eq!(result.output, "");
        assert_eq!(result.response, "\x1b[0n");
        assert_eq!(state.drain_pending(), "beforeafter");
    }

    #[test]
    fn winpty_terminal_output_state_replies_to_cursor_position_report() {
        let mut state = WinptyOutputState::default();
        let result = state.filter_chunk("before\x1b[6nafter");

        assert_eq!(result.output, "");
        assert_eq!(result.response, "\x1b[1;1R");
        assert_eq!(state.drain_pending(), "beforeafter");
    }

    #[test]
    fn terminal_output_renderer_strips_osc_title_sequences() {
        let mut renderer = TerminalOutputRenderer::default();
        let result = renderer.filter_chunk("\x1b]0;C:\\Windows\\system32\\cmd.exe\x07hello \r\n");

        assert_eq!(result.output, "hello\n");
        assert_eq!(result.response, "");
        assert_eq!(renderer.drain_pending(), "");
    }

    #[test]
    fn terminal_output_renderer_handles_split_escape_sequences() {
        let mut renderer = TerminalOutputRenderer::default();

        assert_eq!(renderer.filter_chunk("before\x1b[").output, "before");
        assert_eq!(renderer.filter_chunk("0Kafter").output, "after");
        assert_eq!(renderer.drain_pending(), "");
    }

    #[test]
    fn terminal_output_renderer_applies_backspace_to_utf8_codepoints() {
        let mut renderer = TerminalOutputRenderer::default();

        assert_eq!(
            renderer.filter_chunk("\u{4f60}\u{597d}\x08!\r\n").output,
            "\u{4f60}!\n"
        );
    }

    #[test]
    fn terminal_output_renderer_collapses_winpty_prompt_rewrites() {
        let mut renderer = TerminalOutputRenderer::default();
        let first = renderer
            .filter_chunk(
                "Microsoft Windows XP [\u{7248}\u{672c} 5.1.2600]\x1b[105G(C\r\n)\
                 ) \u{7248}\u{6743}\u{6240}\u{6709} 1985-2001 Microsoft Corp.\r\n\
                 \x1b[107GC:\\chi\r\n>\r",
            )
            .output;
        let second = renderer
            .filter_chunk("\x1b[107GC:\\chi>e\r\ncho hello\x1b[100Ghello\r\n\x1b[107GC:\\chi>\r")
            .output;
        let final_output = first + &second + &renderer.drain_pending();

        assert!(final_output.contains("Microsoft Windows XP [\u{7248}\u{672c} 5.1.2600]"));
        assert!(
            final_output.contains("\u{7248}\u{6743}\u{6240}\u{6709} 1985-2001 Microsoft Corp.")
        );
        assert!(final_output.contains("C:\\chi>"));
        assert!(final_output.contains("hellohello"));
        assert!(
            !final_output
                .contains("                                                                ")
        );
        assert!(!final_output.contains('\x1b'));
    }

    #[test]
    fn terminal_output_renderer_debounces_touched_rows() {
        let mut renderer = TerminalOutputRenderer::new(100, 500);

        assert_eq!(renderer.filter_chunk_at("hello", 1000).output, "");
        assert_eq!(renderer.flush_due_at(1099), "");
        assert_eq!(renderer.flush_due_at(1100), "hello");
        assert_eq!(renderer.drain_pending(), "");
    }

    #[test]
    fn terminal_output_renderer_debounce_uses_latest_repaint() {
        let mut renderer = TerminalOutputRenderer::new(100, 500);

        assert_eq!(
            renderer
                .filter_chunk_at("prompt>echo hello\x1b[116Ghell\r\no", 1000)
                .output,
            ""
        );
        assert_eq!(renderer.flush_due_at(1050), "");
        assert_eq!(
            renderer
                .filter_chunk_at("\r\x1b[1Aprompt>echo hello\x1b[0K\r\nhello", 1050)
                .output,
            ""
        );
        assert_eq!(renderer.flush_due_at(1149), "");

        let emitted = renderer.flush_due_at(1150);
        assert!(emitted.contains("prompt>echo hello"));
        assert!(emitted.contains("hello"));
        assert!(!emitted.contains("hell\no"));
    }

    #[test]
    fn terminal_output_renderer_debounce_drain_flushes_immediately() {
        let mut renderer = TerminalOutputRenderer::new(1000, 2000);

        assert_eq!(renderer.filter_chunk_at("prompt:", 1000).output, "");
        assert_eq!(renderer.drain_pending(), "prompt:");
    }

    #[test]
    fn winpty_output_state_does_not_finalize_partial_rows_on_flush_due() {
        let mut state = WinptyOutputState::default();

        assert_eq!(
            state
                .filter_chunk_at("prompt>echo hello\x1b[116Ghell\r\no", 1000)
                .output,
            ""
        );
        assert_eq!(state.flush_due_at(1100), "");
        assert_eq!(
            state
                .filter_chunk_at("\r\x1b[1Aprompt>echo hello\x1b[0K\r\nhello\r\n", 1150)
                .output,
            ""
        );
        assert_eq!(state.drain_pending(), "prompt>echo hello\nhello\n");
    }
}
