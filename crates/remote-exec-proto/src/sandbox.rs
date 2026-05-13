mod authorize;
mod path_utils;
mod types;

pub use authorize::{authorize_path, compile_filesystem_sandbox};
pub use types::{
    CompiledFilesystemSandbox, FilesystemSandbox, SandboxAccess, SandboxError, SandboxPathList,
};

#[cfg(test)]
mod tests {
    use super::{
        CompiledFilesystemSandbox, FilesystemSandbox, SandboxAccess, SandboxError, SandboxPathList,
        authorize_path, compile_filesystem_sandbox,
    };
    #[cfg(not(windows))]
    use crate::path::linux_path_policy;
    #[cfg(windows)]
    use crate::path::windows_path_policy;

    fn host_path_policy() -> crate::path::PathPolicy {
        #[cfg(windows)]
        {
            windows_path_policy()
        }
        #[cfg(not(windows))]
        {
            linux_path_policy()
        }
    }

    #[test]
    fn authorize_path_rejects_relative_path_with_distinct_error() {
        let sandbox = CompiledFilesystemSandbox::default();
        let err = authorize_path(
            crate::path::linux_path_policy(),
            Some(&sandbox),
            SandboxAccess::Read,
            std::path::Path::new("relative/path"),
        )
        .expect_err("relative path should be rejected");
        assert!(matches!(err, SandboxError::NotAbsolute { .. }));
    }

    #[test]
    fn empty_allow_list_defaults_to_allow_all() {
        let tempdir = tempfile::tempdir().unwrap();
        let nested = tempdir.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        let sandbox = FilesystemSandbox {
            read: SandboxPathList {
                allow: Vec::new(),
                deny: vec![nested.display().to_string()],
            },
            ..Default::default()
        };
        let policy = host_path_policy();
        let compiled = compile_filesystem_sandbox(policy, &sandbox).unwrap();

        assert!(
            authorize_path(
                policy,
                Some(&compiled),
                SandboxAccess::Read,
                &tempdir.path().join("allowed.txt"),
            )
            .is_ok()
        );
        assert!(
            authorize_path(
                policy,
                Some(&compiled),
                SandboxAccess::Read,
                &nested.join("blocked.txt"),
            )
            .is_err()
        );
    }

    #[test]
    fn non_empty_allow_list_requires_membership() {
        let tempdir = tempfile::tempdir().unwrap();
        let allowed = tempdir.path().join("allowed");
        let denied = tempdir.path().join("denied");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&denied).unwrap();

        let sandbox = FilesystemSandbox {
            write: SandboxPathList {
                allow: vec![allowed.display().to_string()],
                deny: Vec::new(),
            },
            ..Default::default()
        };
        let policy = host_path_policy();
        let compiled = compile_filesystem_sandbox(policy, &sandbox).unwrap();

        assert!(
            authorize_path(
                policy,
                Some(&compiled),
                SandboxAccess::Write,
                &allowed.join("ok.txt"),
            )
            .is_ok()
        );
        assert!(
            authorize_path(
                policy,
                Some(&compiled),
                SandboxAccess::Write,
                &denied.join("nope.txt"),
            )
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_case_insensitively() {
        let tempdir = tempfile::tempdir().unwrap();
        let sandbox = FilesystemSandbox {
            read: SandboxPathList {
                allow: vec![tempdir.path().display().to_string().to_uppercase()],
                deny: Vec::new(),
            },
            ..Default::default()
        };
        let compiled = compile_filesystem_sandbox(windows_path_policy(), &sandbox).unwrap();

        assert!(
            authorize_path(
                windows_path_policy(),
                Some(&compiled),
                SandboxAccess::Read,
                &tempdir.path().join("artifact.txt"),
            )
            .is_ok()
        );
    }
}
