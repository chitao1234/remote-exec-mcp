use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use remote_exec_host::{
    path_compare::{path_eq, path_has_prefix, path_is_within},
    sandbox::{SandboxAccess, SandboxError, authorize_path, compile_filesystem_sandbox},
};
use remote_exec_proto::sandbox::{FilesystemSandbox, SandboxPathList};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PathCompareContracts {
    path_eq: Vec<PathEqCase>,
    path_has_prefix: Vec<PathPrefixCase>,
    path_is_within: Vec<PathWithinCase>,
}

#[derive(Debug, Deserialize)]
struct PathEqCase {
    name: String,
    platform: String,
    left: String,
    right: String,
    expected: bool,
}

#[derive(Debug, Deserialize)]
struct PathPrefixCase {
    name: String,
    platform: String,
    path: String,
    prefix: String,
    expected: bool,
}

#[derive(Debug, Deserialize)]
struct PathWithinCase {
    name: String,
    platform: String,
    path: String,
    root: String,
    expected: bool,
}

#[derive(Debug, Deserialize)]
struct SandboxContracts {
    compile: Vec<CompileSandboxCase>,
    authorize: Vec<AuthorizeSandboxCase>,
}

#[derive(Debug, Deserialize)]
struct CompileSandboxCase {
    name: String,
    access: String,
    allow: Vec<String>,
    deny: Vec<String>,
    setup: Option<SetupSpec>,
    expected: String,
    expected_message_fragment: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorizeSandboxCase {
    name: String,
    platforms: Option<Vec<String>>,
    access: String,
    allow: Vec<String>,
    deny: Vec<String>,
    path: String,
    setup: Option<SetupSpec>,
    expected: String,
    expected_message_fragment: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SetupSpec {
    dirs: Option<Vec<String>>,
    files: Option<Vec<FileSpec>>,
    symlinks: Option<Vec<SymlinkSpec>>,
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

fn path_compare_contracts() -> &'static PathCompareContracts {
    static CONTRACTS: OnceLock<PathCompareContracts> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../tests/contracts/path_compare_cases.json"
        ))
        .expect("valid path compare contracts")
    })
}

fn sandbox_contracts() -> &'static SandboxContracts {
    static CONTRACTS: OnceLock<SandboxContracts> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../tests/contracts/sandbox_cases.json"))
            .expect("valid sandbox contracts")
    })
}

fn host_platform_label() -> &'static str {
    if cfg!(windows) { "windows" } else { "posix" }
}

fn sandbox_access(label: &str) -> SandboxAccess {
    match label {
        "exec_cwd" => SandboxAccess::ExecCwd,
        "read" => SandboxAccess::Read,
        "write" => SandboxAccess::Write,
        other => panic!("unknown sandbox access `{other}`"),
    }
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
            create_symlink(
                Path::new(&apply_template(&link.target, root)),
                Path::new(&apply_template(&link.path, root)),
            );
        }
    }
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) {
    use std::os::unix::fs::symlink;

    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    symlink(target, link).unwrap();
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _link: &Path) {
    panic!("symlink setup is only expected on unix hosts in these tests");
}

fn sandbox_for_case(
    access: &str,
    allow: &[String],
    deny: &[String],
    root: &Path,
) -> FilesystemSandbox {
    let list = SandboxPathList {
        allow: allow
            .iter()
            .map(|entry| apply_template(entry, root))
            .collect(),
        deny: deny
            .iter()
            .map(|entry| apply_template(entry, root))
            .collect(),
    };

    let mut sandbox = FilesystemSandbox::default();
    match sandbox_access(access) {
        SandboxAccess::ExecCwd => sandbox.exec_cwd = list,
        SandboxAccess::Read => sandbox.read = list,
        SandboxAccess::Write => sandbox.write = list,
    }
    sandbox
}

fn assert_expected_error(
    case_name: &str,
    error: SandboxError,
    expected: &str,
    expected_message_fragment: Option<&str>,
) {
    match expected {
        "deny" => assert!(matches!(error, SandboxError::Denied { .. }), "{case_name}"),
        "not_absolute" => assert!(
            matches!(error, SandboxError::NotAbsolute { .. }),
            "{case_name}"
        ),
        other => panic!("unknown sandbox error expectation `{other}`"),
    }

    if let Some(fragment) = expected_message_fragment {
        assert!(
            error.to_string().contains(fragment),
            "{case_name}: expected `{fragment}` in `{error}`"
        );
    }
}

#[test]
fn shared_host_path_compare_cases_match() {
    let platform = host_platform_label();
    for case in &path_compare_contracts().path_eq {
        if case.platform == platform {
            assert_eq!(
                path_eq(Path::new(&case.left), Path::new(&case.right)),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    for case in &path_compare_contracts().path_has_prefix {
        if case.platform == platform {
            assert_eq!(
                path_has_prefix(Path::new(&case.path), Path::new(&case.prefix)),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    for case in &path_compare_contracts().path_is_within {
        if case.platform == platform {
            assert_eq!(
                path_is_within(Path::new(&case.path), Path::new(&case.root)),
                case.expected,
                "{}",
                case.name
            );
        }
    }
}

#[test]
fn shared_sandbox_compile_cases_match() {
    for case in &sandbox_contracts().compile {
        let tempdir = tempfile::tempdir().unwrap();
        apply_setup(tempdir.path(), case.setup.as_ref());
        let sandbox = sandbox_for_case(&case.access, &case.allow, &case.deny, tempdir.path());

        match case.expected.as_str() {
            "ok" => {
                compile_filesystem_sandbox(&sandbox).unwrap();
            }
            "compile_error" => {
                let error = compile_filesystem_sandbox(&sandbox).unwrap_err();
                if let Some(fragment) = case.expected_message_fragment.as_deref() {
                    assert!(
                        error.to_string().contains(fragment),
                        "{}: expected `{fragment}` in `{error}`",
                        case.name
                    );
                }
            }
            other => panic!("unknown compile expectation `{other}`"),
        }
    }
}

#[test]
fn shared_sandbox_authorize_cases_match() {
    let platform = host_platform_label();
    for case in &sandbox_contracts().authorize {
        if let Some(platforms) = &case.platforms {
            if !platforms.iter().any(|entry| entry == platform) {
                continue;
            }
        }

        let tempdir = tempfile::tempdir().unwrap();
        apply_setup(tempdir.path(), case.setup.as_ref());
        let sandbox = sandbox_for_case(&case.access, &case.allow, &case.deny, tempdir.path());
        let compiled = compile_filesystem_sandbox(&sandbox).unwrap();
        let path = apply_template(&case.path, tempdir.path());
        let result = authorize_path(
            Some(&compiled),
            sandbox_access(&case.access),
            Path::new(&path),
        );

        match case.expected.as_str() {
            "allow" => {
                result.unwrap();
            }
            "deny" | "not_absolute" => {
                let error = result.unwrap_err();
                assert_expected_error(
                    &case.name,
                    error,
                    &case.expected,
                    case.expected_message_fragment.as_deref(),
                );
            }
            other => panic!("unknown authorize expectation `{other}`"),
        }
    }
}
