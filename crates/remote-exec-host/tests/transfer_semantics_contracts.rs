use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use remote_exec_host::{
    TransferError,
    transfer::archive::{ExportedArchive, export_path_to_archive, import_archive_from_file},
};
use remote_exec_proto::{
    rpc::{TransferImportRequest, TransferOverwrite, TransferSourceType, TransferSymlinkMode},
    transfer::TransferCompression,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct TransferSemanticsContracts {
    import_cases: Vec<ImportCase>,
    export_cases: Vec<ExportCase>,
}

#[derive(Debug, Deserialize)]
struct ImportCase {
    name: String,
    platforms: Option<Vec<String>>,
    source_type: String,
    overwrite: String,
    create_parent: bool,
    symlink_mode: String,
    destination_path: String,
    setup: Option<SetupSpec>,
    archive_entries: Vec<ArchiveEntrySpec>,
    expected: ImportExpected,
}

#[derive(Debug, Deserialize)]
struct ExportCase {
    name: String,
    platforms: Option<Vec<String>>,
    path: String,
    symlink_mode: String,
    setup: Option<SetupSpec>,
    expected: ExportExpected,
}

#[derive(Debug, Deserialize)]
struct SetupSpec {
    dirs: Option<Vec<String>>,
    files: Option<Vec<FileSpec>>,
    symlinks: Option<Vec<SymlinkSpec>>,
    fifos: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct FileSpec {
    path: String,
    contents: String,
}

#[derive(Debug, Deserialize)]
struct SymlinkSpec {
    path: String,
    target: String,
}

#[derive(Debug, Deserialize)]
struct ArchiveEntrySpec {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    contents: Option<String>,
    target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImportExpected {
    replaced: Option<bool>,
    files_copied: Option<u64>,
    directories_copied_at_least: Option<u64>,
    #[serde(default)]
    warning_codes: Vec<String>,
    #[serde(default)]
    file_contents: Vec<FileSpec>,
    #[serde(default)]
    missing_paths: Vec<String>,
    #[serde(default)]
    symlink_targets: Vec<SymlinkSpec>,
    error_message_fragment: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExportExpected {
    source_type: String,
    #[serde(default)]
    archive_paths: Vec<String>,
    #[serde(default)]
    missing_archive_paths: Vec<String>,
    #[serde(default)]
    archive_symlinks: Vec<SymlinkSpec>,
    #[serde(default)]
    roundtrip_warning_codes: Vec<String>,
    #[serde(default)]
    roundtrip_file_contents: Vec<FileSpec>,
    #[serde(default)]
    roundtrip_symlink_targets: Vec<SymlinkSpec>,
    #[serde(default)]
    roundtrip_missing_paths: Vec<String>,
}

fn transfer_semantics_contracts() -> &'static TransferSemanticsContracts {
    static CONTRACTS: OnceLock<TransferSemanticsContracts> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../tests/contracts/transfer_semantics/contract.json"
        ))
        .expect("valid transfer semantics contracts")
    })
}

fn host_platform_label() -> &'static str {
    if cfg!(windows) { "windows" } else { "posix" }
}

fn case_applies(platforms: Option<&Vec<String>>) -> bool {
    let Some(platforms) = platforms else {
        return true;
    };
    platforms.iter().any(|entry| entry == host_platform_label())
}

fn apply_template(raw: &str, root: &Path) -> String {
    raw.replace("{root}", &root.display().to_string())
}

fn apply_setup(root: &Path, setup: Option<&SetupSpec>) {
    let Some(setup) = setup else {
        return;
    };

    if let Some(dirs) = &setup.dirs {
        for dir in dirs {
            fs::create_dir_all(apply_template(dir, root)).unwrap();
        }
    }

    if let Some(files) = &setup.files {
        for file in files {
            let path = PathBuf::from(apply_template(&file.path, root));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, &file.contents).unwrap();
        }
    }

    if let Some(symlinks) = &setup.symlinks {
        for link in symlinks {
            let path = PathBuf::from(apply_template(&link.path, root));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            create_symlink(
                Path::new(&apply_template(&link.target, root)),
                path.as_path(),
            );
        }
    }

    if let Some(fifos) = &setup.fifos {
        for fifo in fifos {
            let path = PathBuf::from(apply_template(fifo, root));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            create_fifo(path.as_path());
        }
    }
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _link: &Path) {
    panic!("symlink setup is only expected on unix hosts in these tests");
}

#[cfg(unix)]
fn create_fifo(path: &Path) {
    use nix::sys::stat::Mode;

    nix::unistd::mkfifo(path, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
}

#[cfg(not(unix))]
fn create_fifo(_path: &Path) {
    panic!("fifo setup is only expected on unix hosts in these tests");
}

fn parse_source_type(label: &str) -> TransferSourceType {
    TransferSourceType::from_wire_value(label)
        .unwrap_or_else(|| panic!("unknown transfer source type `{label}`"))
}

fn parse_overwrite(label: &str) -> TransferOverwrite {
    TransferOverwrite::from_wire_value(label)
        .unwrap_or_else(|| panic!("unknown transfer overwrite `{label}`"))
}

fn parse_symlink_mode(label: &str) -> TransferSymlinkMode {
    TransferSymlinkMode::from_wire_value(label)
        .unwrap_or_else(|| panic!("unknown transfer symlink mode `{label}`"))
}

fn octal_field(width: usize, value: u64) -> Vec<u8> {
    let digits = format!("{value:o}");
    assert!(digits.len() < width, "octal field overflow");
    let mut field = vec![b'0'; width];
    let start = width - 1 - digits.len();
    field[start..start + digits.len()].copy_from_slice(digits.as_bytes());
    field[width - 1] = b' ';
    field
}

fn set_bytes(header: &mut [u8], offset: usize, width: usize, value: &str) {
    let bytes = value.as_bytes();
    let len = bytes.len().min(width);
    header[offset..offset + len].copy_from_slice(&bytes[..len]);
}

fn write_checksum(header: &mut [u8]) {
    header[148..156].fill(b' ');
    let checksum: u32 = header.iter().map(|byte| *byte as u32).sum();
    header[148..156].copy_from_slice(&octal_field(8, checksum as u64));
}

fn append_padded_bytes(archive: &mut Vec<u8>, bytes: &[u8]) {
    archive.extend_from_slice(bytes);
    let remainder = bytes.len() % 512;
    if remainder != 0 {
        archive.resize(archive.len() + (512 - remainder), 0);
    }
}

fn append_tar_entry(
    archive: &mut Vec<u8>,
    path: &str,
    typeflag: u8,
    body: &[u8],
    mode: u64,
    link_target: Option<&str>,
) {
    assert!(
        path.len() <= 100,
        "test tar helper only supports short paths"
    );
    let mut header = vec![0u8; 512];
    set_bytes(&mut header, 0, 100, path);
    header[100..108].copy_from_slice(&octal_field(8, mode));
    header[108..116].copy_from_slice(&octal_field(8, 0));
    header[116..124].copy_from_slice(&octal_field(8, 0));
    header[124..136].copy_from_slice(&octal_field(12, body.len() as u64));
    header[136..148].copy_from_slice(&octal_field(12, 0));
    header[156] = typeflag;
    if let Some(link_target) = link_target {
        set_bytes(&mut header, 157, 100, link_target);
    }
    set_bytes(&mut header, 257, 6, "ustar ");
    set_bytes(&mut header, 263, 2, " \0");
    write_checksum(&mut header);

    archive.extend_from_slice(&header);
    append_padded_bytes(archive, body);
}

fn build_archive(entries: &[ArchiveEntrySpec]) -> Vec<u8> {
    let mut archive = Vec::new();

    for entry in entries {
        match entry.kind.as_str() {
            "directory" => {
                append_tar_entry(&mut archive, &entry.path, b'5', &[], 0o755, None);
            }
            "file" => {
                append_tar_entry(
                    &mut archive,
                    &entry.path,
                    b'0',
                    entry.contents.as_deref().unwrap_or_default().as_bytes(),
                    0o644,
                    None,
                );
            }
            "symlink" => {
                append_tar_entry(
                    &mut archive,
                    &entry.path,
                    b'2',
                    &[],
                    0o777,
                    Some(entry.target.as_deref().expect("symlink target")),
                );
            }
            other => panic!("unknown archive entry type `{other}`"),
        }
    }

    archive.extend_from_slice(&[0u8; 1024]);
    archive
}

fn sorted_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut values: Vec<_> = values.into_iter().collect();
    values.sort();
    values
}

fn sorted_paths_in_archive(bytes: &[u8]) -> Vec<String> {
    let mut archive = tar::Archive::new(io::Cursor::new(bytes));
    let mut paths = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        paths.push(entry.path().unwrap().to_string_lossy().into_owned());
    }
    paths.sort();
    paths
}

fn sorted_archive_symlinks(bytes: &[u8]) -> Vec<(String, String)> {
    let mut archive = tar::Archive::new(io::Cursor::new(bytes));
    let mut links = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        if entry.header().entry_type().is_symlink() {
            links.push((
                entry.path().unwrap().to_string_lossy().into_owned(),
                entry
                    .link_name()
                    .unwrap()
                    .expect("symlink target")
                    .to_string_lossy()
                    .into_owned(),
            ));
        }
    }
    links.sort();
    links
}

fn sorted_expected_symlinks(links: &[SymlinkSpec]) -> Vec<(String, String)> {
    let mut links: Vec<_> = links
        .iter()
        .map(|link| (link.path.clone(), link.target.clone()))
        .collect();
    links.sort();
    links
}

fn sorted_warning_codes(warnings: &[remote_exec_proto::rpc::TransferWarning]) -> Vec<String> {
    let mut codes: Vec<_> = warnings
        .iter()
        .map(|warning| warning.code.clone())
        .collect();
    codes.sort();
    codes
}

fn assert_file_contents(root: &Path, files: &[FileSpec]) {
    for file in files {
        let path = PathBuf::from(apply_template(&file.path, root));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            file.contents,
            "{path:?}"
        );
    }
}

fn assert_missing_paths(root: &Path, paths: &[String]) {
    for path in paths {
        let path = PathBuf::from(apply_template(path, root));
        assert!(!path.exists(), "expected `{}` to be absent", path.display());
    }
}

#[cfg(unix)]
fn assert_symlink_targets(root: &Path, links: &[SymlinkSpec]) {
    for link in links {
        let path = PathBuf::from(apply_template(&link.path, root));
        let expected_target = PathBuf::from(apply_template(&link.target, root));
        assert_eq!(fs::read_link(&path).unwrap(), expected_target, "{path:?}");
    }
}

#[cfg(not(unix))]
fn assert_symlink_targets(_root: &Path, links: &[SymlinkSpec]) {
    assert!(
        links.is_empty(),
        "symlink assertions are only supported on unix"
    );
}

fn roundtrip_destination(root: &Path, source_type: &TransferSourceType) -> PathBuf {
    match source_type {
        TransferSourceType::File => root.join("roundtrip.txt"),
        TransferSourceType::Directory | TransferSourceType::Multiple => root.join("roundtrip"),
    }
}

async fn import_case_response(
    root: &Path,
    case: &ImportCase,
) -> Result<remote_exec_proto::rpc::TransferImportResponse, TransferError> {
    let archive = build_archive(&case.archive_entries);
    let archive_file = tempfile::NamedTempFile::new().unwrap();
    fs::write(archive_file.path(), archive).unwrap();

    let request = TransferImportRequest {
        destination_path: apply_template(&case.destination_path, root),
        overwrite: parse_overwrite(&case.overwrite),
        create_parent: case.create_parent,
        source_type: parse_source_type(&case.source_type),
        compression: TransferCompression::None,
        symlink_mode: parse_symlink_mode(&case.symlink_mode),
    };

    import_archive_from_file(
        archive_file.path(),
        &request,
        None,
        None,
        Default::default(),
    )
    .await
}

async fn export_case_archive(root: &Path, case: &ExportCase) -> ExportedArchive {
    export_path_to_archive(
        &apply_template(&case.path, root),
        TransferCompression::None,
        parse_symlink_mode(&case.symlink_mode),
        &[],
        None,
        None,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn shared_transfer_import_cases_match() {
    for case in &transfer_semantics_contracts().import_cases {
        if !case_applies(case.platforms.as_ref()) {
            continue;
        }

        let tempdir = tempfile::tempdir().unwrap();
        apply_setup(tempdir.path(), case.setup.as_ref());
        let result = import_case_response(tempdir.path(), case).await;

        if let Some(fragment) = case.expected.error_message_fragment.as_deref() {
            let error = result.unwrap_err();
            assert!(
                error.to_string().contains(fragment),
                "{}: expected `{fragment}` in `{error}`",
                case.name
            );
            continue;
        }

        let response = result.unwrap();
        if let Some(replaced) = case.expected.replaced {
            assert_eq!(response.replaced, replaced, "{}", case.name);
        }
        if let Some(files_copied) = case.expected.files_copied {
            assert_eq!(response.files_copied, files_copied, "{}", case.name);
        }
        if let Some(min_directories) = case.expected.directories_copied_at_least {
            assert!(
                response.directories_copied >= min_directories,
                "{}: expected at least {min_directories} directories, got {}",
                case.name,
                response.directories_copied
            );
        }
        assert_eq!(
            sorted_warning_codes(&response.warnings),
            sorted_strings(case.expected.warning_codes.clone()),
            "{}",
            case.name
        );
        assert_file_contents(tempdir.path(), &case.expected.file_contents);
        assert_missing_paths(tempdir.path(), &case.expected.missing_paths);
        assert_symlink_targets(tempdir.path(), &case.expected.symlink_targets);
    }
}

#[tokio::test]
async fn shared_transfer_export_cases_match() {
    for case in &transfer_semantics_contracts().export_cases {
        if !case_applies(case.platforms.as_ref()) {
            continue;
        }

        let tempdir = tempfile::tempdir().unwrap();
        apply_setup(tempdir.path(), case.setup.as_ref());
        let exported = export_case_archive(tempdir.path(), case).await;
        let archive_path = exported.temp_path.to_path_buf();
        let archive_bytes = fs::read(&archive_path).unwrap();
        let archive_paths = sorted_paths_in_archive(&archive_bytes);

        assert_eq!(
            exported.source_type.wire_value(),
            case.expected.source_type,
            "{}",
            case.name
        );
        assert_eq!(
            archive_paths,
            sorted_strings(case.expected.archive_paths.clone()),
            "{}",
            case.name
        );
        for path in &case.expected.missing_archive_paths {
            assert!(
                !archive_paths.iter().any(|entry| entry == path),
                "{}: unexpected archive path `{path}`",
                case.name
            );
        }
        assert_eq!(
            sorted_archive_symlinks(&archive_bytes),
            sorted_expected_symlinks(&case.expected.archive_symlinks),
            "{}",
            case.name
        );
        assert_eq!(
            sorted_warning_codes(&exported.warnings),
            sorted_strings(case.expected.roundtrip_warning_codes.clone()),
            "{}",
            case.name
        );

        let request = TransferImportRequest {
            destination_path: roundtrip_destination(tempdir.path(), &exported.source_type)
                .display()
                .to_string(),
            overwrite: TransferOverwrite::Replace,
            create_parent: true,
            source_type: exported.source_type.clone(),
            compression: exported.compression.clone(),
            symlink_mode: parse_symlink_mode(&case.symlink_mode),
        };
        let roundtrip =
            import_archive_from_file(&archive_path, &request, None, None, Default::default())
                .await
                .unwrap();

        assert_eq!(
            sorted_warning_codes(&roundtrip.warnings),
            sorted_strings(case.expected.roundtrip_warning_codes.clone()),
            "{}",
            case.name
        );
        assert_file_contents(tempdir.path(), &case.expected.roundtrip_file_contents);
        assert_missing_paths(tempdir.path(), &case.expected.roundtrip_missing_paths);
        assert_symlink_targets(tempdir.path(), &case.expected.roundtrip_symlink_targets);
    }
}
