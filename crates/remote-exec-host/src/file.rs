use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use remote_exec_proto::rpc::{
    FileEditRequest, FileEditResponse, FileReadRequest, FileReadResponse, FileWriteRequest,
    FileWriteResponse,
};

use crate::AppState;
use crate::error::FileError;
use crate::host_path::ResolvedHostPath;
use crate::sandbox::SandboxAccess;
use crate::text_file::{TextFile, TextFileError, TextFileErrorKind};

pub async fn read_file_local(
    state: Arc<AppState>,
    req: FileReadRequest,
) -> Result<FileReadResponse, FileError> {
    tracing::info!(
        target = %state.config.target,
        path = %req.path,
        offset = req.offset,
        limit = req.limit,
        max_bytes = req.max_bytes,
        "file read received"
    );

    if req.limit == 0 {
        return Err(FileError::invalid_request(
            "read.limit must be greater than zero",
        ));
    }
    if req.max_bytes == 0 {
        return Err(FileError::invalid_request(
            "read.max_bytes must be greater than zero",
        ));
    }

    let resolved_path = resolve_file_path(&state, &req.path);
    crate::exec::ensure_resolved_sandbox_access(&state, SandboxAccess::Read, &resolved_path)?;
    let path = resolved_path.path();
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|err| metadata_error(path, err))?;
    if !metadata.is_file() {
        return Err(FileError::not_file(format!(
            "file path `{}` is not a file",
            path.display()
        )));
    }
    if metadata.len() > req.max_bytes {
        return Err(FileError::too_large(format!(
            "file `{}` is {} bytes, exceeding max_read_bytes {}",
            path.display(),
            metadata.len(),
            req.max_bytes
        )));
    }

    let text = read_text_file_with_max_bytes(&state, path, req.max_bytes).await?;
    let rendered = render_read_output(&text.text, req.offset, req.limit);
    tracing::info!(
        target = %state.config.target,
        path = %path.display(),
        lines_returned = rendered.lines_returned,
        total_lines = rendered.total_lines,
        eof = rendered.eof,
        "file read completed"
    );

    Ok(FileReadResponse {
        output: rendered.output,
        lines_returned: rendered.lines_returned,
        total_lines: rendered.total_lines,
        eof: rendered.eof,
    })
}

pub async fn write_file_local(
    state: Arc<AppState>,
    req: FileWriteRequest,
) -> Result<FileWriteResponse, FileError> {
    tracing::info!(
        target = %state.config.target,
        path = %req.path,
        content_len = req.content.len(),
        max_bytes = req.max_bytes,
        "file write received"
    );

    if req.max_bytes == 0 {
        return Err(FileError::invalid_request(
            "write.max_bytes must be greater than zero",
        ));
    }

    let resolved_path = resolve_file_path(&state, &req.path);
    crate::exec::ensure_resolved_sandbox_access(&state, SandboxAccess::Write, &resolved_path)?;
    let path = resolved_path.path();
    let target = ensure_writable_file_target(path, req.max_bytes).await?;
    let content =
        encode_for_existing_file(&state, path, &req.content, !target.created, req.max_bytes)
            .await?;
    tokio::fs::write(path, content)
        .await
        .map_err(|err| write_error(path, err))?;
    let line_count = count_lines(&req.content);
    tracing::info!(
        target = %state.config.target,
        path = %path.display(),
        created = target.created,
        line_count,
        "file write completed"
    );

    Ok(FileWriteResponse {
        created: target.created,
        line_count,
    })
}

pub async fn edit_file_local(
    state: Arc<AppState>,
    req: FileEditRequest,
) -> Result<FileEditResponse, FileError> {
    tracing::info!(
        target = %state.config.target,
        path = %req.path,
        old_string_len = req.old_string.len(),
        new_string_len = req.new_string.len(),
        replace_all = req.replace_all,
        max_bytes = req.max_bytes,
        "file edit received"
    );

    if req.max_bytes == 0 {
        return Err(FileError::invalid_request(
            "edit.max_bytes must be greater than zero",
        ));
    }
    if req.old_string.is_empty() {
        return Err(FileError::invalid_request(
            "edit.old_string must not be empty",
        ));
    }

    let resolved_path = resolve_file_path(&state, &req.path);
    crate::exec::ensure_resolved_sandbox_access(&state, SandboxAccess::Write, &resolved_path)?;
    let path = resolved_path.path();
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|err| metadata_error(path, err))?;
    if !metadata.is_file() {
        return Err(FileError::not_file(format!(
            "file path `{}` is not a file",
            path.display()
        )));
    }
    enforce_max_bytes(path, metadata.len(), req.max_bytes)?;

    let text = read_text_file_with_max_bytes(&state, path, req.max_bytes).await?;
    let matches = text.text.match_indices(&req.old_string).count();
    match matches {
        0 => {
            return Err(FileError::old_string_not_found(format!(
                "old_string was not found in `{}`",
                path.display()
            )));
        }
        1 => {}
        count if !req.replace_all => {
            return Err(FileError::old_string_ambiguous(format!(
                "old_string matched {count} times in `{}`; set replace_all to replace every match",
                path.display()
            )));
        }
        _ => {}
    }

    let updated = if req.replace_all {
        text.text.replace(&req.old_string, &req.new_string)
    } else {
        text.text.replacen(&req.old_string, &req.new_string, 1)
    };
    let content = text
        .encode(&updated)
        .map_err(|err| text_write_encoding_error(path, err))?;
    tokio::fs::write(path, content)
        .await
        .map_err(|err| write_error(path, err))?;
    let line_count = count_lines(&updated);
    tracing::info!(
        target = %state.config.target,
        path = %path.display(),
        replacements = matches,
        line_count,
        "file edit completed"
    );

    Ok(FileEditResponse {
        replacements: matches as u64,
        line_count,
    })
}

struct RenderedRead {
    output: String,
    lines_returned: u64,
    total_lines: u64,
    eof: bool,
}

struct WritableFileTarget {
    created: bool,
}

fn resolve_file_path(state: &Arc<AppState>, raw: &str) -> ResolvedHostPath {
    crate::host_path::resolve_input_path_for_operation(
        &state.config.default_workdir,
        raw,
        state.config.windows_posix_root.as_deref(),
    )
}

async fn ensure_writable_file_target(
    path: &Path,
    max_bytes: u64,
) -> Result<WritableFileTarget, FileError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => {
            enforce_max_bytes(path, metadata.len(), max_bytes)?;
            Ok(WritableFileTarget { created: false })
        }
        Ok(_) => Err(FileError::not_file(format!(
            "file path `{}` is not a file",
            path.display()
        ))),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(WritableFileTarget { created: true }),
        Err(err) => Err(FileError::write_failed(format!(
            "unable to access file `{}`: {err}",
            path.display()
        ))),
    }
}

async fn encode_for_existing_file(
    state: &Arc<AppState>,
    path: &Path,
    content: &str,
    existing: bool,
    max_bytes: u64,
) -> Result<Vec<u8>, FileError> {
    if !existing {
        return Ok(content.as_bytes().to_vec());
    }

    let text = read_text_file_with_max_bytes(state, path, max_bytes).await?;
    text.encode(content)
        .map_err(|err| text_write_encoding_error(path, err))
}

async fn read_text_file_with_max_bytes(
    state: &Arc<AppState>,
    path: &Path,
    max_bytes: u64,
) -> Result<TextFile, FileError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|err| text_read_error(path, TextFileError::Io(err)))?;
    enforce_max_bytes(path, bytes.len() as u64, max_bytes)?;
    TextFile::from_bytes(
        bytes,
        state
            .config
            .experimental_apply_patch_target_encoding_autodetect,
    )
    .map_err(|err| text_read_error(path, err))
}

fn render_read_output(text: &str, offset: Option<u64>, limit: u64) -> RenderedRead {
    let start = normalize_offset(offset);
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len() as u64;

    if total_lines == 0 {
        return RenderedRead {
            output: "(file is empty)".to_string(),
            lines_returned: 0,
            total_lines,
            eof: true,
        };
    }

    if start > total_lines {
        return RenderedRead {
            output: format!("(offset out of range, file only has {total_lines} lines)"),
            lines_returned: 0,
            total_lines,
            eof: true,
        };
    }

    let end = total_lines.min(start.saturating_add(limit).saturating_sub(1));
    let mut body = String::new();
    for line_number in start..=end {
        if !body.is_empty() {
            body.push('\n');
        }
        let line = lines[(line_number - 1) as usize];
        body.push_str(&format!("{line_number}: {line}"));
    }

    let eof = end == total_lines;
    let reminder = if eof {
        format!("EOF reached, file has {total_lines} lines")
    } else {
        format!("showing lines {start}-{end} of {total_lines} lines")
    };

    RenderedRead {
        output: format!("{body}\n\n({reminder})"),
        lines_returned: end - start + 1,
        total_lines,
        eof,
    }
}

fn normalize_offset(offset: Option<u64>) -> u64 {
    match offset {
        None | Some(0) => 1,
        Some(value) => value,
    }
}

fn count_lines(text: &str) -> u64 {
    text.lines().count() as u64
}

fn metadata_error(path: &Path, err: std::io::Error) -> FileError {
    if err.kind() == ErrorKind::NotFound {
        FileError::missing(format!("unable to locate file `{}`: {err}", path.display()))
    } else {
        FileError::read_failed(format!("unable to access file `{}`: {err}", path.display()))
    }
}

fn enforce_max_bytes(path: &Path, len: u64, max_bytes: u64) -> Result<(), FileError> {
    if len > max_bytes {
        return Err(FileError::too_large(format!(
            "file `{}` is {len} bytes, exceeding max_read_bytes {max_bytes}",
            path.display()
        )));
    }
    Ok(())
}

fn text_read_error(path: &Path, err: TextFileError) -> FileError {
    match err.kind() {
        TextFileErrorKind::Io => {
            FileError::read_failed(format!("unable to read file `{}`: {err}", path.display()))
        }
        TextFileErrorKind::Decode | TextFileErrorKind::Encode | TextFileErrorKind::Binary => {
            FileError::decode_failed(format!(
                "unable to decode file `{}` as text: {err}",
                path.display()
            ))
        }
    }
}

fn text_write_encoding_error(path: &Path, err: TextFileError) -> FileError {
    FileError::write_failed(format!(
        "unable to encode updated text for file `{}`: {err}",
        path.display()
    ))
}

fn write_error(path: &Path, err: std::io::Error) -> FileError {
    FileError::write_failed(format!("unable to write file `{}`: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{count_lines, render_read_output};

    #[test]
    fn read_offset_zero_is_first_line() {
        let rendered = render_read_output("a\nb\nc\n", Some(0), 2);

        assert_eq!(
            rendered.output,
            "1: a\n2: b\n\n(showing lines 1-2 of 3 lines)"
        );
        assert_eq!(rendered.lines_returned, 2);
        assert_eq!(rendered.total_lines, 3);
        assert!(!rendered.eof);
    }

    #[test]
    fn read_reports_eof_when_limit_reaches_end() {
        let rendered = render_read_output("a\nb\n", Some(2), 10);

        assert_eq!(rendered.output, "2: b\n\n(EOF reached, file has 2 lines)");
        assert_eq!(rendered.lines_returned, 1);
        assert_eq!(rendered.total_lines, 2);
        assert!(rendered.eof);
    }

    #[test]
    fn empty_file_has_zero_lines() {
        let rendered = render_read_output("", None, 10);

        assert_eq!(rendered.output, "(file is empty)");
        assert_eq!(rendered.lines_returned, 0);
        assert_eq!(rendered.total_lines, 0);
        assert!(rendered.eof);
        assert_eq!(count_lines(""), 0);
        assert_eq!(count_lines("a\n"), 1);
        assert_eq!(count_lines("a\n\n"), 2);
    }
}
