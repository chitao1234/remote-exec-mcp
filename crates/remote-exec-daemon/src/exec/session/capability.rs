use portable_pty::{NativePtySystem, PtySize, PtySystem};

use crate::config::{PtyMode, WindowsPtyBackendOverride};

pub(super) fn default_pty_size() -> PtySize {
    PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub(super) fn portable_pty_probe() -> anyhow::Result<()> {
    NativePtySystem::default()
        .openpty(default_pty_size())
        .map(|_| ())
}

pub fn supports_pty_with_override(
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
) -> bool {
    #[cfg(windows)]
    {
        super::windows::supports_pty_with_override(windows_pty_backend_override)
    }

    #[cfg(not(windows))]
    {
        let _ = windows_pty_backend_override;
        portable_pty_probe().is_ok()
    }
}

pub fn supports_pty() -> bool {
    supports_pty_with_override(None)
}

pub fn windows_pty_backend_override_for_mode(
    pty_mode: PtyMode,
) -> anyhow::Result<Option<WindowsPtyBackendOverride>> {
    match pty_mode {
        PtyMode::Auto | PtyMode::None => Ok(None),
        PtyMode::Conpty => {
            #[cfg(windows)]
            {
                Ok(Some(WindowsPtyBackendOverride::PortablePty))
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("configured PTY backend `conpty` is only supported on Windows");
            }
        }
        PtyMode::Winpty => {
            #[cfg(windows)]
            {
                Ok(Some(WindowsPtyBackendOverride::Winpty))
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("configured PTY backend `winpty` is only supported on Windows");
            }
        }
    }
}

pub fn supports_pty_for_mode(pty_mode: PtyMode) -> bool {
    if matches!(pty_mode, PtyMode::None) {
        return false;
    }

    let Ok(windows_pty_backend_override) = windows_pty_backend_override_for_mode(pty_mode) else {
        return false;
    };
    supports_pty_with_override(windows_pty_backend_override)
}

pub fn validate_pty_mode(pty_mode: PtyMode) -> anyhow::Result<()> {
    if matches!(pty_mode, PtyMode::Auto | PtyMode::None) {
        return Ok(());
    }

    anyhow::ensure!(
        supports_pty_for_mode(pty_mode),
        "configured PTY backend `{}` is not available on this host",
        match pty_mode {
            PtyMode::Conpty => "conpty",
            PtyMode::Winpty => "winpty",
            PtyMode::Auto | PtyMode::None => unreachable!("validated above"),
        }
    );
    Ok(())
}
