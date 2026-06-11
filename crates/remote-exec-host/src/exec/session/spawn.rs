use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};

use portable_pty::{CommandBuilder, NativePtySystem, PtySystem};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
#[cfg(windows)]
use windows_sys::Win32::Foundation::ERROR_INVALID_FLAGS;
#[cfg(windows)]
use windows_sys::Win32::Globalization::{
    CPINFO, GetACP, GetCPInfo, GetOEMCP, MB_ERR_INVALID_CHARS, MultiByteToWideChar,
};

use crate::config::{ProcessEnvironment, WindowsPtyBackendOverride};

use super::capability::default_pty_size;
#[cfg(not(windows))]
use super::capability::supports_pty;
use super::child::{PtySession, SessionChild};
use super::live::{LiveSession, new_live_session};

const PIPE_OUTPUT_READ_BUFFER_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCommand {
    pub program: String,
    pub argv0: Option<String>,
    pub args: Vec<String>,
    pub windows_raw_arg_tail: Option<String>,
}

impl SpawnCommand {
    pub fn from_argv(argv: &[String]) -> anyhow::Result<Self> {
        anyhow::ensure!(!argv.is_empty(), "spawn command argv must not be empty");
        Ok(Self {
            program: argv[0].clone(),
            argv0: None,
            args: argv[1..].to_vec(),
            windows_raw_arg_tail: None,
        })
    }

    pub fn argv(&self) -> Vec<String> {
        std::iter::once(self.program.clone())
            .chain(self.args.iter().cloned())
            .collect()
    }

    pub fn windows_command_line(&self) -> String {
        if let Some(raw_arg_tail) = &self.windows_raw_arg_tail {
            let mut line = quote_windows_argument(&self.program);
            if !raw_arg_tail.is_empty() {
                line.push(' ');
                line.push_str(raw_arg_tail);
            }
            line
        } else {
            command_line_from_argv(&self.argv())
        }
    }
}

fn command_line_from_argv(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_windows_argument(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_windows_argument(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    if !arg.chars().any(|ch| matches!(ch, ' ' | '\t' | '"')) {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;

    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }

    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

pub fn spawn_with_windows_pty_backend_override(
    cmd: &SpawnCommand,
    cwd: &std::path::Path,
    tty: bool,
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    if tty {
        #[cfg(windows)]
        {
            super::windows::spawn_tty_session(cmd, cwd, windows_pty_backend_override, environment)
        }

        #[cfg(not(windows))]
        {
            let _ = windows_pty_backend_override;
            anyhow::ensure!(supports_pty(), "tty is not supported on this host");
            spawn_pty(cmd, cwd, environment)
        }
    } else {
        spawn_pipe(cmd, cwd, environment)
    }
}

pub fn spawn(
    cmd: &[String],
    cwd: &std::path::Path,
    tty: bool,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let cmd = SpawnCommand::from_argv(cmd)?;
    spawn_with_windows_pty_backend_override(&cmd, cwd, tty, None, environment)
}

#[cfg(windows)]
pub async fn windows_pty_debug_report(cmd: &SpawnCommand, cwd: &std::path::Path) -> String {
    super::windows::debug_report(cmd, cwd).await
}

pub(super) fn spawn_pty(
    cmd: &SpawnCommand,
    cwd: &std::path::Path,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let pty = NativePtySystem::default().openpty(default_pty_size())?;
    let mut builder = CommandBuilder::new(&cmd.program);
    if let Some(argv0) = &cmd.argv0 {
        builder.arg0(argv0);
    }
    #[cfg(windows)]
    if let Some(raw_arg_tail) = &cmd.windows_raw_arg_tail {
        builder.raw_arg(raw_arg_tail);
    } else {
        for arg in &cmd.args {
            builder.arg(arg);
        }
    }
    #[cfg(not(windows))]
    {
        for arg in &cmd.args {
            builder.arg(arg);
        }
    }
    builder.cwd(cwd);
    super::environment::apply_overlay_builder(&mut builder, environment);

    let child = pty.slave.spawn_command(builder)?;
    let writer = pty.master.take_writer()?;
    let reader = pty.master.try_clone_reader()?;
    let (sender, receiver) = unbounded_channel();
    spawn_output_reader(reader, sender);

    Ok(new_live_session(
        true,
        SessionChild::Pty(PtySession {
            child,
            master: pty.master,
            writer,
        }),
        receiver,
    ))
}

fn spawn_pipe(
    cmd: &SpawnCommand,
    cwd: &std::path::Path,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let (reader, writer) = os_pipe::pipe()?;
    let stderr = writer.try_clone()?;
    let mut command = Command::new(&cmd.program);
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::from(stderr));
    #[cfg(windows)]
    if let Some(raw_arg_tail) = &cmd.windows_raw_arg_tail {
        command.raw_arg(raw_arg_tail);
    } else {
        command.args(&cmd.args);
    }
    #[cfg(not(windows))]
    command.args(&cmd.args);
    #[cfg(unix)]
    if let Some(argv0) = &cmd.argv0 {
        command.arg0(argv0);
    }
    super::environment::apply_overlay_std_command(&mut command, environment);
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            let result = nix::libc::setpgid(0, 0);
            if result == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let child = command.spawn()?;
    let (sender, receiver) = unbounded_channel();
    let session = new_live_session(false, SessionChild::Pipe(Box::new(child)), receiver);

    spawn_output_reader(reader, sender);

    Ok(session)
}

fn spawn_output_reader<R>(mut reader: R, sender: UnboundedSender<String>)
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0u8; PIPE_OUTPUT_READ_BUFFER_SIZE];
        let mut decoder = PipeDecoder::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    if let Some(chunk) = decoder.finish() {
                        let _ = sender.send(chunk);
                    }
                    break;
                }
                Ok(read) => {
                    let Some(chunk) = decoder.push(&buffer[..read]) else {
                        continue;
                    };
                    if sender.send(chunk).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    if let Some(chunk) = decoder.finish() {
                        let _ = sender.send(chunk);
                    }
                    break;
                }
            }
        }
    });
}

#[cfg(not(windows))]
type PipeDecoder = Utf8PipeDecoder;

#[cfg(windows)]
type PipeDecoder = WindowsPipeDecoder;

#[cfg(any(not(windows), test))]
struct Utf8PipeDecoder {
    pending: Vec<u8>,
}

#[cfg(any(not(windows), test))]
impl Utf8PipeDecoder {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        decode_utf8_lossy_stream_chunk(&mut self.pending, bytes, false)
    }

    fn finish(&mut self) -> Option<String> {
        decode_utf8_lossy_stream_chunk(&mut self.pending, &[], true)
    }
}

#[cfg(windows)]
struct WindowsPipeDecoder {
    primary_code_page: u32,
    fallback_code_page: u32,
    pending: Vec<u8>,
}

#[cfg(windows)]
impl WindowsPipeDecoder {
    fn new() -> Self {
        unsafe { Self::with_code_pages(GetOEMCP(), GetACP()) }
    }

    fn with_code_pages(primary_code_page: u32, fallback_code_page: u32) -> Self {
        Self {
            primary_code_page,
            fallback_code_page,
            pending: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        self.decode_chunk(bytes, false)
    }

    fn finish(&mut self) -> Option<String> {
        self.decode_chunk(&[], true)
    }

    fn decode_chunk(&mut self, bytes: &[u8], flush: bool) -> Option<String> {
        self.pending.extend_from_slice(bytes);
        if self.pending.is_empty() {
            return None;
        }

        let mut raw = std::mem::take(&mut self.pending);
        if !flush {
            carry_incomplete_windows_code_page_suffix(
                self.primary_code_page,
                &mut raw,
                &mut self.pending,
            );
            if raw.is_empty() {
                return None;
            }
        }

        let output = match utf8_from_windows_code_page(self.primary_code_page, &raw) {
            Ok(output) => output,
            Err(primary_err) => {
                log_windows_console_ansi_fallback_once(&primary_err);
                match utf8_from_windows_code_page(self.fallback_code_page, &raw) {
                    Ok(output) => output,
                    Err(fallback_err) => {
                        log_windows_console_utf8_fallback_once(&fallback_err);
                        raw.extend_from_slice(&self.pending);
                        self.pending.clear();
                        return decode_utf8_lossy_stream_chunk(&mut self.pending, &raw, flush);
                    }
                }
            }
        };
        Some(output)
    }
}

#[cfg(windows)]
fn carry_incomplete_windows_code_page_suffix(
    code_page: u32,
    raw: &mut Vec<u8>,
    carry: &mut Vec<u8>,
) {
    let Some(max_char_size) = windows_code_page_max_char_size(code_page) else {
        return;
    };

    if windows_code_page_decodes(code_page, raw) {
        return;
    }

    let max_suffix = raw.len().min(max_char_size - 1);
    for suffix_size in 1..=max_suffix {
        let prefix_size = raw.len() - suffix_size;
        if prefix_size == 0 || windows_code_page_decodes(code_page, &raw[..prefix_size]) {
            carry.extend_from_slice(&raw[prefix_size..]);
            raw.truncate(prefix_size);
            return;
        }
    }
}

#[cfg(windows)]
fn windows_code_page_max_char_size(code_page: u32) -> Option<usize> {
    let mut info = CPINFO {
        MaxCharSize: 0,
        DefaultChar: [0; 2],
        LeadByte: [0; 12],
    };
    if unsafe { GetCPInfo(code_page, &mut info) } == 0 {
        return None;
    }
    let max_char_size = info.MaxCharSize as usize;
    (max_char_size > 1).then_some(max_char_size)
}

#[cfg(windows)]
fn windows_code_page_decodes(code_page: u32, raw: &[u8]) -> bool {
    if raw.is_empty() {
        return true;
    }

    match wide_len_from_windows_code_page(code_page, MB_ERR_INVALID_CHARS, raw) {
        Ok(_) => true,
        Err(err) if err.raw_os_error() == Some(ERROR_INVALID_FLAGS as i32) => {
            wide_len_from_windows_code_page(code_page, 0, raw).is_ok()
        }
        Err(_) => false,
    }
}

#[cfg(windows)]
static LOGGED_WINDOWS_CONSOLE_ANSI_FALLBACK: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static LOGGED_WINDOWS_CONSOLE_UTF8_FALLBACK: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
fn log_windows_console_ansi_fallback_once(err: &std::io::Error) {
    if LOGGED_WINDOWS_CONSOLE_ANSI_FALLBACK
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        tracing::warn!(
            error = %err,
            "Windows pipe output OEM decode failed; falling back to ANSI code page"
        );
    }
}

#[cfg(windows)]
fn log_windows_console_utf8_fallback_once(err: &std::io::Error) {
    if LOGGED_WINDOWS_CONSOLE_UTF8_FALLBACK
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        tracing::warn!(
            error = %err,
            "Windows pipe output ANSI decode failed; falling back to UTF-8 replacement decoding"
        );
    }
}

#[cfg(windows)]
fn utf8_from_windows_code_page(code_page: u32, raw: &[u8]) -> std::io::Result<String> {
    if raw.is_empty() {
        return Ok(String::new());
    }

    match wide_from_windows_code_page(code_page, MB_ERR_INVALID_CHARS, raw) {
        Ok(wide) => Ok(String::from_utf16_lossy(&wide)),
        Err(err) if err.raw_os_error() == Some(ERROR_INVALID_FLAGS as i32) => {
            let wide = wide_from_windows_code_page(code_page, 0, raw)?;
            Ok(String::from_utf16_lossy(&wide))
        }
        Err(err) => Err(err),
    }
}

#[cfg(windows)]
fn wide_from_windows_code_page(
    code_page: u32,
    flags: u32,
    raw: &[u8],
) -> std::io::Result<Vec<u16>> {
    let wide_len = wide_len_from_windows_code_page(code_page, flags, raw)?;
    let mut wide = vec![0u16; wide_len as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            raw.as_ptr(),
            raw.len() as i32,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    if written <= 0 {
        return Err(std::io::Error::last_os_error());
    }
    wide.truncate(written as usize);
    Ok(wide)
}

#[cfg(windows)]
fn wide_len_from_windows_code_page(code_page: u32, flags: u32, raw: &[u8]) -> std::io::Result<i32> {
    let raw_len = i32::try_from(raw.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Windows pipe output chunk is too large to decode",
        )
    })?;

    let wide_len = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            raw.as_ptr(),
            raw_len,
            std::ptr::null_mut(),
            0,
        )
    };
    if wide_len <= 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(wide_len)
}

#[cfg(test)]
mod tests {
    use super::{SpawnCommand, command_line_from_argv, quote_windows_argument};

    #[test]
    fn quote_windows_argument_leaves_simple_arguments_unchanged() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument(r#"C:\Tools\bin"#), r#"C:\Tools\bin"#);
    }

    #[test]
    fn quote_windows_argument_quotes_whitespace_and_embedded_quotes() {
        assert_eq!(quote_windows_argument("two words"), r#""two words""#);
        assert_eq!(
            quote_windows_argument(r#"quote "mark""#),
            r#""quote \"mark\"""#
        );
    }

    #[test]
    fn quote_windows_argument_doubles_trailing_backslashes_before_closing_quote() {
        assert_eq!(
            quote_windows_argument(r#"C:\Program Files\Test Folder\"#),
            r#""C:\Program Files\Test Folder\\""#,
        );
    }

    #[test]
    fn command_line_from_argv_quotes_each_argument_for_windows_spawn() {
        assert_eq!(
            command_line_from_argv(&[
                "bash.exe".to_string(),
                "-c".to_string(),
                "printf ok".to_string(),
            ]),
            r#"bash.exe -c "printf ok""#
        );
    }

    #[test]
    fn command_line_from_argv_quotes_whole_argv_for_windows_spawn() {
        assert_eq!(
            command_line_from_argv(&[
                "pwsh.exe".to_string(),
                "plain".to_string(),
                "two words".to_string(),
                r#"quote "mark""#.to_string(),
                r#"C:\Program Files\Test Folder\"#.to_string(),
            ]),
            r#"pwsh.exe plain "two words" "quote \"mark\"" "C:\Program Files\Test Folder\\""#,
        );
    }

    #[test]
    fn windows_command_line_appends_raw_arg_tail_verbatim() {
        let cmd = SpawnCommand {
            program: "cmd.exe".to_string(),
            argv0: None,
            args: vec![
                "/D".to_string(),
                "/C".to_string(),
                r#"cmd /c "echo hello world""#.to_string(),
            ],
            windows_raw_arg_tail: Some(r#"/D /C cmd /c "echo hello world""#.to_string()),
        };

        assert_eq!(
            cmd.windows_command_line(),
            r#"cmd.exe /D /C cmd /c "echo hello world""#
        );
    }
}

fn complete_utf8_lossy_prefix_len(bytes: &[u8]) -> usize {
    let mut offset = 0;
    loop {
        match std::str::from_utf8(&bytes[offset..]) {
            Ok(_) => return bytes.len(),
            Err(err) => {
                let invalid_at = offset + err.valid_up_to();
                match err.error_len() {
                    Some(error_len) => offset = invalid_at + error_len,
                    None => return invalid_at,
                }
            }
        }
    }
}

fn decode_utf8_lossy_stream_chunk(
    pending: &mut Vec<u8>,
    bytes: &[u8],
    flush: bool,
) -> Option<String> {
    pending.extend_from_slice(bytes);
    if pending.is_empty() {
        return None;
    }

    let complete_len = if flush {
        pending.len()
    } else {
        complete_utf8_lossy_prefix_len(pending)
    };
    if complete_len == 0 {
        return None;
    }

    let output = String::from_utf8_lossy(&pending[..complete_len]).into_owned();
    pending.drain(..complete_len);
    Some(output)
}

#[cfg(test)]
mod exec_pipe_decoder_tests {
    use super::Utf8PipeDecoder;
    #[cfg(windows)]
    use super::{WindowsPipeDecoder, utf8_from_windows_code_page};
    #[cfg(windows)]
    use windows_sys::Win32::Globalization::{CP_UTF8, WideCharToMultiByte};

    #[test]
    fn split_multibyte_codepoint_is_emitted_once() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(decoder.push(&[0xe4, 0xbd]), None);
        assert_eq!(decoder.push(&[0xa0]), Some("你".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn invalid_complete_sequence_is_lossy_but_trailing_prefix_is_preserved() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(
            decoder.push(&[0xff, b'a', 0xf0, 0x9f]),
            Some("\u{fffd}a".to_string())
        );
        assert_eq!(decoder.push(&[0x98, 0x80]), Some("😀".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn unfinished_sequence_is_replaced_on_finish() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(decoder.push(&[b'a', 0xe4, 0xbd]), Some("a".to_string()));
        assert_eq!(decoder.finish(), Some("\u{fffd}".to_string()));
    }

    #[cfg(windows)]
    fn code_page_available(code_page: u32) -> bool {
        utf8_from_windows_code_page(code_page, b"x").is_ok()
    }

    #[cfg(windows)]
    fn bytes_from_windows_code_page(code_page: u32, wide: &[u16]) -> Option<Vec<u8>> {
        if wide.is_empty() {
            return Some(Vec::new());
        }

        let wide_len = i32::try_from(wide.len()).ok()?;
        let raw_len = unsafe {
            WideCharToMultiByte(
                code_page,
                0,
                wide.as_ptr(),
                wide_len,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if raw_len <= 0 {
            return None;
        }

        let mut raw = vec![0u8; raw_len as usize];
        let written = unsafe {
            WideCharToMultiByte(
                code_page,
                0,
                wide.as_ptr(),
                wide_len,
                raw.as_mut_ptr(),
                raw_len,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if written <= 0 {
            return None;
        }
        raw.truncate(written as usize);
        Some(raw)
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_decoder_carries_split_dbcs_character() {
        if !code_page_available(936) {
            return;
        }

        let mut decoder = WindowsPipeDecoder::with_code_pages(936, 1252);

        assert_eq!(decoder.push(&[0xc4]), None);
        assert_eq!(decoder.push(&[0xe3]), Some("你".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_decoder_carries_split_utf8_when_console_code_page_is_utf8() {
        let mut decoder = WindowsPipeDecoder::with_code_pages(CP_UTF8, 1252);

        assert_eq!(decoder.push(&[0xe4, 0xbd]), None);
        assert_eq!(decoder.push(&[0xa0]), Some("你".to_string()));
        assert_eq!(decoder.finish(), None);

        let mut decoder = WindowsPipeDecoder::with_code_pages(CP_UTF8, 1252);

        assert_eq!(decoder.push(&[0xf0]), None);
        assert_eq!(decoder.push(&[0x9f, 0x98]), None);
        assert_eq!(decoder.push(&[0x80]), Some("😀".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_decoder_carries_split_four_byte_code_page_sequence() {
        if !code_page_available(54936) {
            return;
        }

        let Some(raw) = bytes_from_windows_code_page(54936, &[0xd83d, 0xde00]) else {
            return;
        };
        if raw.len() != 4 {
            return;
        }

        let mut decoder = WindowsPipeDecoder::with_code_pages(54936, 1252);

        assert_eq!(decoder.push(&raw[..2]), None);
        assert_eq!(decoder.push(&raw[2..3]), None);
        assert_eq!(decoder.push(&raw[3..]), Some("😀".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_decoder_falls_back_to_ansi_code_page() {
        if !code_page_available(1252) {
            return;
        }

        let mut decoder = WindowsPipeDecoder::with_code_pages(99999, 1252);

        assert_eq!(decoder.push(b"caf\xe9"), Some("café".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_decoder_falls_back_to_utf8_with_replacement() {
        let mut decoder = WindowsPipeDecoder::with_code_pages(99999, 99998);

        assert_eq!(
            decoder.push(b"caf\xc3\xa9 ok\xff"),
            Some("café ok\u{fffd}".to_string())
        );
        assert_eq!(decoder.finish(), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_decoder_carries_split_utf8_during_utf8_fallback() {
        let mut decoder = WindowsPipeDecoder::with_code_pages(99999, 99998);

        assert_eq!(decoder.push(&[0xe4, 0xbd]), None);
        assert_eq!(decoder.push(&[0xa0]), Some("你".to_string()));
        assert_eq!(decoder.finish(), None);
    }
}
