mod support;

use encoding_rs::{BIG5, EUC_KR, Encoding, GBK, SHIFT_JIS};
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse};

fn utf16le_bom_bytes(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(|unit| unit.to_le_bytes()));
    bytes
}

fn encoded_bytes(encoding: &'static Encoding, text: &str) -> Vec<u8> {
    let (encoded, _, had_errors) = encoding.encode(text);
    assert!(
        !had_errors,
        "test text should be encodable as {}",
        encoding.name()
    );
    encoded.into_owned()
}

#[tokio::test]
async fn add_file_overwrites_existing_content() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("demo.txt");
    tokio::fs::write(&path, "old\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Add File: demo.txt\n+new\n*** End Patch\n".to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("Success."));
    assert!(
        response
            .daemon_instance_id
            .as_deref()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(response.updated_paths, vec!["A demo.txt"]);
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "new\n");
}

#[tokio::test]
async fn update_file_preserves_crlf_line_endings() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("crlf.txt");
    tokio::fs::write(&path, b"hello\r\nworld\r\n")
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: crlf.txt\n",
                    "@@\n",
                    "-hello\n",
                    "+hello daemon\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M crlf.txt"));
    assert_eq!(
        tokio::fs::read(path).await.unwrap(),
        b"hello daemon\r\nworld\r\n"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn apply_patch_accepts_msys_style_workdir_on_windows() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let workdir = fixture.workdir.join("msys-workdir");
    tokio::fs::create_dir_all(&workdir).await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Add File: demo.txt\n+new\n*** End Patch\n".to_string(),
                workdir: Some(support::msys_style_path(&workdir)),
            },
        )
        .await;

    assert!(response.output.contains("A demo.txt"));
    assert_eq!(
        tokio::fs::read_to_string(workdir.join("demo.txt"))
            .await
            .unwrap(),
        "new\n"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn apply_patch_accepts_cygwin_style_absolute_paths_on_windows() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("cygdrive-demo.txt");

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: format!(
                    "*** Begin Patch\n*** Add File: {}\n+new\n*** End Patch\n",
                    support::cygwin_style_path(&path)
                ),
                workdir: None,
            },
        )
        .await;

    assert!(response.output.contains("A cygdrive-demo.txt"));
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "new\n");
}

#[cfg(windows)]
#[tokio::test]
async fn apply_patch_accepts_windows_posix_root_paths_on_windows() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config_for_workdir("builder-a", |workdir| {
            let root = workdir.join("synthetic-msys-root");
            format!(
                "windows_posix_root = {}\n",
                toml::Value::String(root.display().to_string())
            )
        })
        .await;
    let root = fixture.workdir.join("synthetic-msys-root");
    let workdir = root.join("usr").join("src");
    tokio::fs::create_dir_all(&workdir).await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Add File: demo.txt\n+new\n*** End Patch\n".to_string(),
                workdir: Some("/usr/src".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("A demo.txt"));
    assert_eq!(
        tokio::fs::read_to_string(workdir.join("demo.txt"))
            .await
            .unwrap(),
        "new\n"
    );
}

#[tokio::test]
async fn update_file_accepts_end_of_file_marker() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "before\nmiddle\nbefore\n")
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** End of File\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M plain.txt"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "before\nmiddle\nafter\n",
    );
}

#[tokio::test]
async fn update_file_replaces_blank_last_real_line_at_end_of_file() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "alpha\n\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@\n",
                    "-\n",
                    "+omega\n",
                    "*** End of File\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M plain.txt"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "alpha\nomega\n"
    );
}

#[tokio::test]
async fn update_file_accepts_first_chunk_without_explicit_header() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "before\nmiddle\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "-before\n",
                    "+after\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M plain.txt"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "after\nmiddle\n",
    );
}

#[tokio::test]
async fn update_file_matches_old_lines_ignoring_trailing_whitespace() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "alpha  \ntail\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@\n",
                    "-alpha\n",
                    "+omega\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M plain.txt"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "omega\ntail\n",
    );
}

#[tokio::test]
async fn update_file_matches_change_context_after_unicode_normalization() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "start\nalpha — “beta\u{00a0}gamma”\ntail\n")
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@ alpha - \"beta gamma\"\n",
                    "+inserted\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M plain.txt"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "start\ninserted\nalpha — “beta\u{00a0}gamma”\ntail\n"
    );
}

#[tokio::test]
async fn update_file_matches_old_lines_after_unicode_normalization() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "start\nalpha — “beta\u{00a0}gamma”\ntail\n")
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@\n",
                    "-alpha - \"beta gamma\"\n",
                    "+omega\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M plain.txt"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "start\nomega\ntail\n",
    );
}

#[tokio::test]
async fn update_file_rejects_singleton_empty_eof_hunk_without_trailing_newline() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "alpha").await.unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@\n",
                    "-\n",
                    "+omega\n",
                    "*** End of File\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "alpha");
}

#[tokio::test]
async fn update_move_accepts_horizontal_whitespace_on_control_lines() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("old.txt");
    let destination = fixture.workdir.join("new.txt");
    tokio::fs::write(&source, "old\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    " \t*** Begin Patch\t\n",
                    "\t*** Update File: old.txt  \n",
                    "  *** Move to: new.txt\t\n",
                    " \t@@\t\n",
                    "-old\n",
                    "+new\n",
                    "\t*** End of File \n",
                    "  *** End Patch\t\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M new.txt"));
    assert_eq!(
        tokio::fs::read_to_string(destination).await.unwrap(),
        "new\n"
    );
    assert!(tokio::fs::metadata(source).await.is_err());
}

#[tokio::test]
async fn update_file_rejects_non_eof_match_for_end_of_file_marker() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "before\nmiddle\ntail\n")
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** End of File\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "before\nmiddle\ntail\n",
    );
}

#[tokio::test]
async fn update_file_appends_at_eof_for_pure_addition_with_matching_context() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "before\ntail\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@ tail\n",
                    "+after\n",
                    "*** End of File\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M plain.txt"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "before\ntail\nafter\n",
    );
}

#[tokio::test]
async fn update_file_rejects_eof_pure_addition_when_context_is_missing() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "before\ntail\n").await.unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@ missing\n",
                    "+after\n",
                    "*** End of File\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "before\ntail\n",
    );
}

#[tokio::test]
async fn later_failures_leave_earlier_files_mutated() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Delete File: missing.txt\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "after\n",
    );
}

#[tokio::test]
async fn delete_directory_failure_leaves_earlier_mutation_applied() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();
    tokio::fs::create_dir(fixture.workdir.join("nested"))
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Delete File: nested\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "after\n",
    );
}

#[tokio::test]
async fn non_utf8_update_source_failure_leaves_earlier_mutation_applied() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();
    tokio::fs::write(fixture.workdir.join("binary.txt"), vec![0xff, 0xfe, 0xfd])
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Update File: binary.txt\n",
                    "@@\n",
                    "-old\n",
                    "+new\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "after\n",
    );
}

#[tokio::test]
async fn non_utf8_delete_source_failure_leaves_earlier_mutation_applied() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();
    tokio::fs::write(fixture.workdir.join("binary.txt"), vec![0xff, 0xfe, 0xfd])
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Delete File: binary.txt\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "after\n",
    );
}

#[tokio::test]
async fn utf16le_update_source_still_fails_when_autodetect_is_disabled() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("utf16.txt");
    tokio::fs::write(&path, utf16le_bom_bytes("hello\r\nworld\r\n"))
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: utf16.txt\n",
                    "@@\n",
                    "-hello\n",
                    "+hello daemon\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read(path).await.unwrap(),
        utf16le_bom_bytes("hello\r\nworld\r\n")
    );
}

#[tokio::test]
async fn update_file_autodetects_utf16le_target_encoding_when_enabled() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        "experimental_apply_patch_target_encoding_autodetect = true",
    )
    .await;
    let path = fixture.workdir.join("utf16.txt");
    tokio::fs::write(&path, utf16le_bom_bytes("hello\r\nworld\r\n"))
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: utf16.txt\n",
                    "@@\n",
                    "-hello\n",
                    "+hello daemon\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M utf16.txt"));
    assert_eq!(
        tokio::fs::read(path).await.unwrap(),
        utf16le_bom_bytes("hello daemon\r\nworld\r\n")
    );
}

#[tokio::test]
async fn delete_file_autodetects_utf16le_target_encoding_when_enabled() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        "experimental_apply_patch_target_encoding_autodetect = true",
    )
    .await;
    let path = fixture.workdir.join("utf16.txt");
    tokio::fs::write(&path, utf16le_bom_bytes("hello\r\n"))
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Delete File: utf16.txt\n*** End Patch\n".to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("D utf16.txt"));
    assert!(tokio::fs::metadata(path).await.is_err());
}

#[tokio::test]
async fn update_file_autodetects_common_east_asian_encodings_when_enabled() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        "experimental_apply_patch_target_encoding_autodetect = true",
    )
    .await;

    let cases = [
        (
            "shift-jis.txt",
            SHIFT_JIS,
            "価格を更新します。\r\n次の行です。\r\n",
            "価格を反映します。\r\n次の行です。\r\n",
            "価格を更新します。",
            "価格を反映します。",
        ),
        (
            "gbk.txt",
            GBK,
            "简体中文文件。\r\n第二行内容。\r\n",
            "简体中文配置。\r\n第二行内容。\r\n",
            "简体中文文件。",
            "简体中文配置。",
        ),
        (
            "big5.txt",
            BIG5,
            "繁體中文檔案。\r\n第二行內容。\r\n",
            "繁體中文設定。\r\n第二行內容。\r\n",
            "繁體中文檔案。",
            "繁體中文設定。",
        ),
        (
            "euc-kr.txt",
            EUC_KR,
            "한국어 파일입니다.\r\n둘째 줄입니다.\r\n",
            "한국어 설정입니다.\r\n둘째 줄입니다.\r\n",
            "한국어 파일입니다.",
            "한국어 설정입니다.",
        ),
    ];

    for (filename, encoding, original_text, updated_text, old_line, new_line) in cases {
        let path = fixture.workdir.join(filename);
        tokio::fs::write(&path, encoded_bytes(encoding, original_text))
            .await
            .unwrap();

        let response = fixture
            .rpc::<PatchApplyRequest, PatchApplyResponse>(
                "/v1/patch/apply",
                &PatchApplyRequest {
                    patch: format!(
                        concat!(
                            "*** Begin Patch\n",
                            "*** Update File: {filename}\n",
                            "@@\n",
                            "-{old_line}\n",
                            "+{new_line}\n",
                            "*** End Patch\n",
                        ),
                        filename = filename,
                        old_line = old_line,
                        new_line = new_line,
                    ),
                    workdir: Some(".".to_string()),
                },
            )
            .await;

        assert!(response.output.contains(&format!("M {filename}")));
        assert_eq!(
            tokio::fs::read(&path).await.unwrap(),
            encoded_bytes(encoding, updated_text),
            "failed to preserve {}",
            encoding.name()
        );
    }
}

#[tokio::test]
async fn add_file_overwrite_preserves_utf16le_target_encoding_when_enabled() {
    let fixture = support::spawn::spawn_daemon_with_extra_config(
        "builder-a",
        "experimental_apply_patch_target_encoding_autodetect = true",
    )
    .await;
    let path = fixture.workdir.join("utf16.txt");
    tokio::fs::write(&path, utf16le_bom_bytes("before\r\n"))
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Add File: utf16.txt\n+after\n*** End Patch\n"
                    .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("A utf16.txt"));
    assert_eq!(
        tokio::fs::read(path).await.unwrap(),
        utf16le_bom_bytes("after\n")
    );
}

#[tokio::test]
async fn execution_failures_do_not_roll_back_earlier_file_changes() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n")
        .await
        .unwrap();
    tokio::fs::write(fixture.workdir.join("blocked"), "not a directory\n")
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Add File: blocked/second.txt\n",
                    "+hello\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt"))
            .await
            .unwrap(),
        "after\n",
    );
    assert!(std::fs::metadata(fixture.workdir.join("blocked/second.txt")).is_err());
}

#[tokio::test]
async fn update_file_applies_repeated_context_additions_in_order() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("plain.txt");
    tokio::fs::write(&path, "a\nmarker\nb\nmarker\nc\n")
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: plain.txt\n",
                    "@@ marker\n",
                    "+first\n",
                    "@@ marker\n",
                    "+second\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("M plain.txt"));
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "a\nfirst\nmarker\nb\nsecond\nmarker\nc\n",
    );
}

#[tokio::test]
async fn apply_patch_uses_resolved_paths_not_workdir_for_sandbox_checks() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config_for_workdir("builder-a", |workdir| {
            let allow = toml::Value::Array(vec![toml::Value::String(
                workdir.join("visible").display().to_string(),
            )]);
            format!(
                r#"[sandbox.write]
allow = {allow}
"#
            )
        })
        .await;
    tokio::fs::create_dir_all(fixture.workdir.join("visible"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(fixture.workdir.join("blocked"))
        .await
        .unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Add File: ../visible/demo.txt\n+ok\n*** End Patch\n"
                    .to_string(),
                workdir: Some("blocked".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("A ../visible/demo.txt"));
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("visible/demo.txt"))
            .await
            .unwrap(),
        "ok\n"
    );
}

#[tokio::test]
async fn apply_patch_rejects_writes_outside_sandbox() {
    let fixture =
        support::spawn::spawn_daemon_with_extra_config_for_workdir("builder-a", |workdir| {
            let allow = toml::Value::Array(vec![toml::Value::String(
                workdir.join("visible").display().to_string(),
            )]);
            format!(
                r#"[sandbox.write]
allow = {allow}
"#
            )
        })
        .await;
    tokio::fs::create_dir_all(fixture.workdir.join("blocked"))
        .await
        .unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Add File: blocked/nope.txt\n+nope\n*** End Patch\n"
                    .to_string(),
                workdir: None,
            },
        )
        .await;

    assert_eq!(err.code, "sandbox_denied");
    assert!(err.message.contains("write access"));
    assert!(
        tokio::fs::metadata(fixture.workdir.join("blocked/nope.txt"))
            .await
            .is_err()
    );
}
