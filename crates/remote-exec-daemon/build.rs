use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(manifest_dir);
    let date = build_date();
    let hash = git_hash(repository_dir);
    let version = match hash {
        Some(hash) => format!("{}+g{hash}.{date}", env!("CARGO_PKG_VERSION")),
        None => format!("{}+{date}", env!("CARGO_PKG_VERSION")),
    };

    println!("cargo:rustc-env=REMOTE_EXEC_BUILD_VERSION={version}");
}

fn git_hash(repository_dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-C",
            &repository_dir.to_string_lossy(),
            "rev-parse",
            "--short=12",
            "HEAD",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8(output.stdout).ok()?;
    let hash = hash.trim();
    (!hash.is_empty()).then(|| hash.to_owned())
}

fn build_date() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("build time must be after Unix epoch")
        .as_secs()
        / 86_400;
    let (year, month, day) = civil_date(days as i64);
    format!("{year:04}{month:02}{day:02}")
}

// Gregorian calendar conversion from days since 1970-01-01.
fn civil_date(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year, month, day)
}
