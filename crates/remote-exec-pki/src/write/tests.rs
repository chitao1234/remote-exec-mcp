use super::write_text_file;

#[test]
fn write_text_file_replaces_existing_file_without_pre_remove() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.key");
    std::fs::write(&path, "old").expect("old file");

    let mut written = Vec::new();
    write_text_file(&path, "new", 0o600, &mut written).expect("replace file");

    assert_eq!(std::fs::read_to_string(&path).expect("read file"), "new");
}

#[cfg(unix)]
#[test]
fn write_text_file_sets_key_permissions_after_rename() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.key");

    let mut written = Vec::new();
    write_text_file(&path, "secret", 0o600, &mut written).expect("write file");

    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(windows)]
#[test]
fn write_text_file_sets_protected_private_key_acl() {
    use std::{iter, os::windows::ffi::OsStrExt, path::Path, ptr};

    use windows_sys::Win32::{
        Foundation::{ERROR_SUCCESS, LocalFree},
        Security::{
            ACL,
            Authorization::{
                EXPLICIT_ACCESS_W, GRANT_ACCESS, GetExplicitEntriesFromAclW, GetNamedSecurityInfoW,
                INHERITED_ACCESS_ENTRY, SE_FILE_OBJECT, TRUSTEE_IS_SID,
            },
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, PSECURITY_DESCRIPTOR,
            SE_DACL_PROTECTED,
        },
    };

    struct LocalPtr(*mut core::ffi::c_void);

    impl Drop for LocalPtr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = LocalFree(self.0);
                }
            }
        }
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(iter::once(0))
            .collect()
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.key");

    let mut written = Vec::new();
    write_text_file(&path, "secret", 0o600, &mut written).expect("write file");

    let path_wide = wide_path(&path);
    let mut dacl = ptr::null_mut::<ACL>();
    let mut descriptor = ptr::null_mut::<core::ffi::c_void>();
    let result = unsafe {
        GetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(result, ERROR_SUCCESS);
    let _descriptor = LocalPtr(descriptor);

    let mut control = 0_u16;
    let mut revision = 0_u32;
    let control_read = unsafe {
        GetSecurityDescriptorControl(
            descriptor as PSECURITY_DESCRIPTOR,
            &mut control,
            &mut revision,
        )
    };
    assert_ne!(control_read, 0);
    assert_eq!(control & SE_DACL_PROTECTED, SE_DACL_PROTECTED);

    let mut entry_count = 0_u32;
    let mut entries = ptr::null_mut::<EXPLICIT_ACCESS_W>();
    let entries_read = unsafe { GetExplicitEntriesFromAclW(dacl, &mut entry_count, &mut entries) };
    assert_eq!(entries_read, ERROR_SUCCESS);
    let _entries = LocalPtr(entries.cast());

    let entries = unsafe { std::slice::from_raw_parts(entries, entry_count as usize) };
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|entry| {
        entry.grfAccessMode == GRANT_ACCESS
            && entry.Trustee.TrusteeForm == TRUSTEE_IS_SID
            && (entry.grfInheritance & INHERITED_ACCESS_ENTRY) == 0
    }));
}
