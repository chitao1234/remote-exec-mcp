use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use crate::{
    generate::GeneratedDevInitBundle,
    manifest::{DevInitManifest, KeyPairPaths, build_manifest},
    spec::DevInitSpec,
};

pub const CA_CERT_FILENAME: &str = "ca.pem";
pub const CA_KEY_FILENAME: &str = "ca.key";

pub fn write_ca_pair(
    pair: &crate::GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    let paths = KeyPairPaths {
        cert_pem: out_dir.join(CA_CERT_FILENAME),
        key_pem: out_dir.join(CA_KEY_FILENAME),
    };
    write_pair(&paths, pair, force)?;
    Ok(paths)
}

pub fn write_broker_pair(
    pair: &crate::GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    let paths = KeyPairPaths {
        cert_pem: out_dir.join("broker.pem"),
        key_pem: out_dir.join("broker.key"),
    };
    write_pair(&paths, pair, force)?;
    Ok(paths)
}

pub fn write_daemon_pair(
    target: &str,
    pair: &crate::GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    let paths = KeyPairPaths {
        cert_pem: out_dir.join(format!("{target}.pem")),
        key_pem: out_dir.join(format!("{target}.key")),
    };
    write_pair(&paths, pair, force)?;
    Ok(paths)
}

pub fn write_dev_init_bundle(
    spec: &DevInitSpec,
    bundle: &GeneratedDevInitBundle,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<DevInitManifest> {
    let daemon_out_dir = out_dir.join("daemons");
    fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    fs::create_dir_all(&daemon_out_dir)
        .with_context(|| format!("creating {}", daemon_out_dir.display()))?;

    let ca = KeyPairPaths {
        cert_pem: out_dir.join(CA_CERT_FILENAME),
        key_pem: out_dir.join(CA_KEY_FILENAME),
    };
    let broker = KeyPairPaths {
        cert_pem: out_dir.join("broker.pem"),
        key_pem: out_dir.join("broker.key"),
    };
    let manifest_path = out_dir.join("certs-manifest.json");
    let daemon_paths = build_daemon_paths(spec, &daemon_out_dir);
    validate_dev_init_output_paths(&ca, &broker, &daemon_paths, &manifest_path, force)?;

    let mut written_paths = Vec::new();
    write_generated_pair(&ca, &bundle.ca, force, &mut written_paths)?;
    write_generated_pair(&broker, &bundle.broker, force, &mut written_paths)?;
    write_generated_daemon_pairs(spec, bundle, &daemon_paths, force, &mut written_paths)?;

    let manifest = build_manifest(spec, out_dir.to_path_buf(), ca, broker, daemon_paths)?;
    write_tracked_text_file(
        &manifest_path,
        &serde_json::to_string_pretty(&manifest)?,
        force,
        0o644,
        &mut written_paths,
    )?;

    Ok(manifest)
}

fn build_daemon_paths(spec: &DevInitSpec, daemon_out_dir: &Path) -> BTreeMap<String, KeyPairPaths> {
    spec.daemon_specs
        .iter()
        .map(|daemon| {
            let target = daemon.target.clone();
            (
                target.clone(),
                KeyPairPaths {
                    cert_pem: daemon_out_dir.join(format!("{target}.pem")),
                    key_pem: daemon_out_dir.join(format!("{target}.key")),
                },
            )
        })
        .collect()
}

fn validate_dev_init_output_paths(
    ca: &KeyPairPaths,
    broker: &KeyPairPaths,
    daemon_paths: &BTreeMap<String, KeyPairPaths>,
    manifest_path: &Path,
    force: bool,
) -> anyhow::Result<()> {
    validate_output_paths(
        std::iter::once(ca.cert_pem.as_path())
            .chain(std::iter::once(ca.key_pem.as_path()))
            .chain(std::iter::once(broker.cert_pem.as_path()))
            .chain(std::iter::once(broker.key_pem.as_path()))
            .chain(
                daemon_paths
                    .values()
                    .flat_map(|paths| [paths.cert_pem.as_path(), paths.key_pem.as_path()]),
            )
            .chain(std::iter::once(manifest_path)),
        force,
    )
}

fn write_generated_daemon_pairs(
    spec: &DevInitSpec,
    bundle: &GeneratedDevInitBundle,
    daemon_paths: &BTreeMap<String, KeyPairPaths>,
    force: bool,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    for daemon in &spec.daemon_specs {
        let target = daemon.target.as_str();
        let pem_pair = bundle
            .daemons
            .get(target)
            .with_context(|| format!("missing generated daemon bundle for `{target}`"))?;
        let paths = daemon_paths
            .get(target)
            .with_context(|| format!("missing validated daemon output paths for `{target}`"))?;
        write_generated_pair(paths, pem_pair, force, written_paths)?;
    }
    Ok(())
}

fn validate_output_paths<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    force: bool,
) -> anyhow::Result<()> {
    for path in paths {
        if path.exists() && !force {
            bail!(
                "output path already exists: {} (rerun with --force to overwrite)",
                path.display()
            );
        }
    }
    Ok(())
}

fn write_pair(
    paths: &KeyPairPaths,
    pair: &crate::GeneratedPemPair,
    force: bool,
) -> anyhow::Result<()> {
    validate_output_paths([paths.cert_pem.as_path(), paths.key_pem.as_path()], force)?;

    let mut written_paths = Vec::new();
    write_generated_pair(paths, pair, force, &mut written_paths)
}

fn write_generated_pair(
    paths: &KeyPairPaths,
    pair: &crate::GeneratedPemPair,
    force: bool,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    write_tracked_text_file(&paths.cert_pem, &pair.cert_pem, force, 0o644, written_paths)?;
    write_tracked_text_file(
        &paths.key_pem,
        pair.key_pem.as_str(),
        force,
        0o600,
        written_paths,
    )?;
    Ok(())
}

fn write_tracked_text_file(
    path: &Path,
    contents: &str,
    force: bool,
    mode: u32,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    write_text_file(path, contents, force, mode, written_paths)
        .map_err(|err| err.context(format_written_paths(written_paths)))
}

fn write_text_file(
    path: &Path,
    contents: &str,
    force: bool,
    mode: u32,
    written_paths: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if path.exists() && !force {
        bail!(
            "output path already exists: {} (rerun with --force to overwrite)",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let tmp_path = temporary_path(path);
    fs::write(&tmp_path, contents).with_context(|| format!("writing {}", tmp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::rename(&tmp_path, path)
            .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("setting permissions on {}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        // Windows ACL hardening is not implemented here yet; mode is preserved
        // in the API so Unix callers can enforce private key permissions.
        let _ = mode;
        if path.exists() {
            fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
        }

        fs::rename(&tmp_path, path)
            .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;
    }
    written_paths.push(path.to_path_buf());
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("remote-exec-pki-output"));
    path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()))
}

fn format_written_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "previously written paths: none".to_string()
    } else {
        format!(
            "previously written paths: {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::write_text_file;

    #[test]
    fn write_text_file_replaces_existing_file_without_pre_remove() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ca.key");
        std::fs::write(&path, "old").expect("old file");

        let mut written = Vec::new();
        write_text_file(&path, "new", true, 0o600, &mut written).expect("replace file");

        assert_eq!(std::fs::read_to_string(&path).expect("read file"), "new");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn write_text_file_does_not_delete_destination_before_replace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ca.key");
        std::fs::write(&path, "old").expect("old file");

        let watch = linux_inotify::DirectoryWatch::new(dir.path());
        let mut written = Vec::new();
        write_text_file(&path, "new", true, 0o600, &mut written).expect("replace file");

        assert_eq!(std::fs::read_to_string(&path).expect("read file"), "new");
        assert!(
            !watch.saw_delete_for("ca.key"),
            "destination was deleted before replacement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_text_file_sets_key_permissions_after_rename() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ca.key");

        let mut written = Vec::new();
        write_text_file(&path, "secret", false, 0o600, &mut written).expect("write file");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(target_os = "linux")]
    mod linux_inotify {
        use std::{
            ffi::CString,
            os::raw::{c_char, c_int, c_void},
            path::Path,
        };

        const IN_CLOSE_WRITE: u32 = 0x0000_0008;
        const IN_CREATE: u32 = 0x0000_0100;
        const IN_DELETE: u32 = 0x0000_0200;
        const IN_MOVED_FROM: u32 = 0x0000_0040;
        const IN_MOVED_TO: u32 = 0x0000_0080;
        const IN_ATTRIB: u32 = 0x0000_0004;
        const IN_NONBLOCK: c_int = 0x0000_0800;
        const IN_CLOEXEC: c_int = 0x0008_0000;
        const EVENT_HEADER_LEN: usize = 16;

        unsafe extern "C" {
            fn inotify_init1(flags: c_int) -> c_int;
            fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: u32) -> c_int;
            fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
            fn close(fd: c_int) -> c_int;
        }

        pub struct DirectoryWatch {
            fd: c_int,
        }

        impl DirectoryWatch {
            pub fn new(path: &Path) -> Self {
                let path = CString::new(path.as_os_str().to_string_lossy().as_bytes())
                    .expect("watch path contains no NUL");
                let fd = unsafe { inotify_init1(IN_NONBLOCK | IN_CLOEXEC) };
                assert!(fd >= 0, "inotify_init1 failed");

                let mask = IN_CLOSE_WRITE
                    | IN_CREATE
                    | IN_DELETE
                    | IN_MOVED_FROM
                    | IN_MOVED_TO
                    | IN_ATTRIB;
                let watch = unsafe { inotify_add_watch(fd, path.as_ptr(), mask) };
                assert!(watch >= 0, "inotify_add_watch failed");

                Self { fd }
            }

            pub fn saw_delete_for(&self, expected_name: &str) -> bool {
                self.events()
                    .into_iter()
                    .any(|event| event.name == expected_name && (event.mask & IN_DELETE) != 0)
            }

            fn events(&self) -> Vec<Event> {
                let mut buffer = [0_u8; 4096];
                let size =
                    unsafe { read(self.fd, buffer.as_mut_ptr().cast::<c_void>(), buffer.len()) };
                if size <= 0 {
                    return Vec::new();
                }

                let mut offset = 0_usize;
                let mut events = Vec::new();
                let size = size as usize;
                while offset + EVENT_HEADER_LEN <= size {
                    let mask = u32::from_ne_bytes(
                        buffer[offset + 4..offset + 8]
                            .try_into()
                            .expect("mask bytes"),
                    );
                    let name_len = u32::from_ne_bytes(
                        buffer[offset + 12..offset + 16]
                            .try_into()
                            .expect("name length bytes"),
                    ) as usize;
                    let name_start = offset + EVENT_HEADER_LEN;
                    let name_end = name_start + name_len;
                    if name_end > size {
                        break;
                    }
                    let raw_name = &buffer[name_start..name_end];
                    let nul = raw_name
                        .iter()
                        .position(|byte| *byte == 0)
                        .unwrap_or(raw_name.len());
                    let name = String::from_utf8_lossy(&raw_name[..nul]).into_owned();
                    events.push(Event { mask, name });
                    offset = name_end;
                }
                events
            }
        }

        impl Drop for DirectoryWatch {
            fn drop(&mut self) {
                unsafe {
                    let _ = close(self.fd);
                }
            }
        }

        struct Event {
            mask: u32,
            name: String,
        }
    }
}
