use std::{
    fs::File, io::Write, iter, mem, os::windows::ffi::OsStrExt, os::windows::io::FromRawHandle,
    path::Path, ptr,
};

use anyhow::{Context, bail};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE, LocalFree,
    },
    Security::{
        ACL,
        Authorization::{
            EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT, SetEntriesInAclW,
            SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
        },
        CreateWellKnownSid, DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation,
        InitializeSecurityDescriptor, PROTECTED_DACL_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED,
        SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SECURITY_MAX_SID_SIZE,
        SetSecurityDescriptorControl, SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER,
        TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    },
    Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
const PRIVATE_KEY_MODE: u32 = 0o600;

pub fn write_text_file(path: &Path, contents: &str, mode: u32) -> anyhow::Result<()> {
    if mode != PRIVATE_KEY_MODE {
        std::fs::write(path, contents)?;
        return Ok(());
    }

    if path.exists() {
        std::fs::remove_file(path)?;
    }

    let acl = PrivateKeyAcl::new()?;
    let mut security_descriptor = SecurityDescriptor::with_dacl(acl.as_ptr())?;
    let security_attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security_descriptor.as_mut_ptr().cast(),
        bInheritHandle: 0,
    };

    let path_wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            FILE_GENERIC_WRITE,
            0,
            &security_attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        bail!("CreateFileW failed with Windows error {}", unsafe {
            GetLastError()
        });
    }

    let mut file = unsafe { File::from_raw_handle(handle) };
    file.write_all(contents.as_bytes())?;
    Ok(())
}

pub fn harden_path_if_private_key(path: &Path, mode: u32) -> anyhow::Result<()> {
    if mode == PRIVATE_KEY_MODE {
        set_private_key_acl(path)?;
    }
    Ok(())
}

fn set_private_key_acl(path: &Path) -> anyhow::Result<()> {
    let acl = PrivateKeyAcl::new()?;
    let path_wide = wide_path(path);
    let result = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl.as_ptr(),
            ptr::null(),
        )
    };
    if result != ERROR_SUCCESS {
        bail!("SetNamedSecurityInfoW failed with Windows error {result}");
    }
    Ok(())
}

struct PrivateKeyAcl {
    acl: *mut ACL,
    _current_user: Vec<u8>,
    _administrators: Vec<u8>,
    _local_system: Vec<u8>,
}

impl PrivateKeyAcl {
    fn new() -> anyhow::Result<Self> {
        let current_user = current_user_sid().context("reading current user SID")?;
        let administrators =
            well_known_sid(WinBuiltinAdministratorsSid).context("building Administrators SID")?;
        let local_system = well_known_sid(WinLocalSystemSid).context("building LocalSystem SID")?;
        let entries = [
            allow_entry(sid_from_bytes(&current_user), FILE_ALL_ACCESS),
            allow_entry(sid_from_bytes(&administrators), FILE_ALL_ACCESS),
            allow_entry(sid_from_bytes(&local_system), FILE_ALL_ACCESS),
        ];
        let mut acl = ptr::null_mut();
        let result = unsafe {
            SetEntriesInAclW(
                entries.len() as u32,
                entries.as_ptr(),
                ptr::null(),
                &mut acl,
            )
        };
        if result != ERROR_SUCCESS {
            bail!("SetEntriesInAclW failed with Windows error {result}");
        }
        Ok(Self {
            acl,
            _current_user: current_user,
            _administrators: administrators,
            _local_system: local_system,
        })
    }

    fn as_ptr(&self) -> *mut ACL {
        self.acl
    }
}

impl Drop for PrivateKeyAcl {
    fn drop(&mut self) {
        if !self.acl.is_null() {
            unsafe {
                let _ = LocalFree(self.acl.cast());
            }
        }
    }
}

struct SecurityDescriptor(SECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn with_dacl(acl: *mut ACL) -> anyhow::Result<Self> {
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let initialized = unsafe {
            InitializeSecurityDescriptor(
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
                SECURITY_DESCRIPTOR_REVISION,
            )
        };
        if initialized == 0 {
            bail!(
                "InitializeSecurityDescriptor failed with Windows error {}",
                unsafe { GetLastError() }
            );
        }

        let dacl_set = unsafe {
            SetSecurityDescriptorDacl(
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
                1,
                acl,
                0,
            )
        };
        if dacl_set == 0 {
            bail!(
                "SetSecurityDescriptorDacl failed with Windows error {}",
                unsafe { GetLastError() }
            );
        }

        let protected = unsafe {
            SetSecurityDescriptorControl(
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
                SE_DACL_PROTECTED,
                SE_DACL_PROTECTED,
            )
        };
        if protected == 0 {
            bail!(
                "SetSecurityDescriptorControl failed with Windows error {}",
                unsafe { GetLastError() }
            );
        }

        Ok(Self(descriptor))
    }

    fn as_mut_ptr(&mut self) -> *mut SECURITY_DESCRIPTOR {
        &mut self.0
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn current_user_sid() -> anyhow::Result<Vec<u8>> {
    let mut token = ptr::null_mut();
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        bail!("OpenProcessToken failed with Windows error {}", unsafe {
            GetLastError()
        });
    }
    let token = Handle(token);

    let mut required_len = 0_u32;
    let first =
        unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut required_len) };
    let first_error = unsafe { GetLastError() };
    if first != 0 || first_error != ERROR_INSUFFICIENT_BUFFER {
        bail!("GetTokenInformation size query failed with Windows error {first_error}");
    }

    let mut buffer = vec![0_u8; required_len as usize];
    let read = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required_len,
            &mut required_len,
        )
    };
    if read == 0 {
        bail!("GetTokenInformation failed with Windows error {}", unsafe {
            GetLastError()
        });
    }

    let token_user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    copy_sid(token_user.User.Sid)
}

fn well_known_sid(kind: i32) -> anyhow::Result<Vec<u8>> {
    let mut required_len = SECURITY_MAX_SID_SIZE;
    let mut sid = vec![0_u8; required_len as usize];
    let created = unsafe {
        CreateWellKnownSid(
            kind,
            ptr::null_mut(),
            sid.as_mut_ptr().cast(),
            &mut required_len,
        )
    };
    if created == 0 {
        bail!("CreateWellKnownSid failed with Windows error {}", unsafe {
            GetLastError()
        });
    }
    sid.truncate(required_len as usize);
    Ok(sid)
}

fn copy_sid(sid: PSID) -> anyhow::Result<Vec<u8>> {
    let sid_len = unsafe { GetLengthSid(sid) };
    if sid_len == 0 {
        bail!("GetLengthSid failed with Windows error {}", unsafe {
            GetLastError()
        });
    }

    let mut copied = vec![0_u8; sid_len as usize];
    unsafe {
        ptr::copy_nonoverlapping(sid.cast::<u8>(), copied.as_mut_ptr(), copied.len());
    }
    Ok(copied)
}

fn sid_from_bytes(sid: &[u8]) -> PSID {
    sid.as_ptr().cast_mut().cast()
}

fn allow_entry(sid: PSID, access: u32) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.cast(),
        },
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect()
}
