use std::path::Path;

use remote_exec_proto::rpc::TransferSourceType;

pub const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

pub struct ExportedArchive {
    pub source_type: TransferSourceType,
    pub temp_path: tempfile::TempPath,
}

pub async fn export_path_to_archive(path: &Path) -> anyhow::Result<ExportedArchive> {
    anyhow::ensure!(
        path.is_absolute(),
        "transfer source path `{}` is not absolute",
        path.display()
    );

    let metadata = tokio::fs::symlink_metadata(path).await?;
    let source_type = if metadata.file_type().is_symlink() {
        anyhow::bail!("transfer source contains unsupported symlink `{}`", path.display());
    } else if metadata.file_type().is_file() {
        TransferSourceType::File
    } else if metadata.file_type().is_dir() {
        TransferSourceType::Directory
    } else {
        anyhow::bail!(
            "transfer source path `{}` is not a regular file or directory",
            path.display()
        );
    };

    let temp = tempfile::NamedTempFile::new()?;
    let temp_path = temp.into_temp_path();
    let archive_path = temp_path.to_path_buf();
    let source_path = path.to_path_buf();
    let source_type_for_task = source_type.clone();

    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = std::fs::File::create(&archive_path)?;
        let mut builder = tar::Builder::new(file);

        match source_type_for_task {
            TransferSourceType::File => {
                builder.append_path_with_name(&source_path, SINGLE_FILE_ENTRY)?;
            }
            TransferSourceType::Directory => {
                builder.append_dir(".", &source_path)?;
                append_directory_entries(&mut builder, &source_path, &source_path)?;
            }
        }

        builder.finish()?;
        Ok(())
    })
    .await??;

    Ok(ExportedArchive {
        source_type,
        temp_path,
    })
}

fn append_directory_entries(
    builder: &mut tar::Builder<std::fs::File>,
    root: &Path,
    current: &Path,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "transfer source contains unsupported symlink `{}`",
            path.display()
        );
        if metadata.is_dir() {
            builder.append_dir(rel, &path)?;
            append_directory_entries(builder, root, &path)?;
        } else if metadata.is_file() {
            builder.append_path_with_name(&path, rel)?;
        } else {
            anyhow::bail!("transfer source contains unsupported entry `{}`", path.display());
        }
    }

    Ok(())
}
