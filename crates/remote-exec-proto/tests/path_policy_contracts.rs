use std::sync::OnceLock;

use remote_exec_proto::path::{
    PathPolicy, basename_for_policy, is_absolute_for_policy, join_for_policy, linux_path_policy,
    normalize_for_system, syntax_eq_for_policy, windows_path_policy,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PathPolicyContracts {
    is_absolute: Vec<AbsoluteCase>,
    normalize_for_system: Vec<NormalizeCase>,
    syntax_eq: Vec<SyntaxEqCase>,
    join: Vec<JoinCase>,
    basename: Vec<BasenameCase>,
}

#[derive(Debug, Deserialize)]
struct AbsoluteCase {
    name: String,
    style: String,
    raw: String,
    expected: bool,
}

#[derive(Debug, Deserialize)]
struct NormalizeCase {
    name: String,
    style: String,
    raw: String,
    expected: String,
}

#[derive(Debug, Deserialize)]
struct SyntaxEqCase {
    name: String,
    style: String,
    left: String,
    right: String,
    expected: bool,
}

#[derive(Debug, Deserialize)]
struct JoinCase {
    name: String,
    style: String,
    base: String,
    child: String,
    expected: String,
}

#[derive(Debug, Deserialize)]
struct BasenameCase {
    name: String,
    style: String,
    raw: String,
    expected: Option<String>,
}

fn contracts() -> &'static PathPolicyContracts {
    static CONTRACTS: OnceLock<PathPolicyContracts> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../tests/contracts/path_policy_cases.json"
        ))
        .expect("valid path policy contracts")
    })
}

fn policy(style: &str) -> PathPolicy {
    match style {
        "posix" => linux_path_policy(),
        "windows" => windows_path_policy(),
        other => panic!("unknown path policy style `{other}`"),
    }
}

#[test]
fn shared_path_policy_absolute_cases_match() {
    for case in &contracts().is_absolute {
        assert_eq!(
            is_absolute_for_policy(policy(&case.style), &case.raw),
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn shared_path_policy_normalization_cases_match() {
    for case in &contracts().normalize_for_system {
        assert_eq!(
            normalize_for_system(policy(&case.style), &case.raw),
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn shared_path_policy_syntax_eq_cases_match() {
    for case in &contracts().syntax_eq {
        assert_eq!(
            syntax_eq_for_policy(policy(&case.style), &case.left, &case.right),
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn shared_path_policy_join_cases_match() {
    for case in &contracts().join {
        assert_eq!(
            join_for_policy(policy(&case.style), &case.base, &case.child),
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn shared_path_policy_basename_cases_match() {
    for case in &contracts().basename {
        assert_eq!(
            basename_for_policy(policy(&case.style), &case.raw),
            case.expected,
            "{}",
            case.name
        );
    }
}
