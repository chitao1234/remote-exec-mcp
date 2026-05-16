#![allow(
    dead_code,
    reason = "This shared test helper is compiled independently in broker and daemon test crates"
)]

use std::io::{Cursor, Read};
use std::path::Path;

pub const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

pub fn decode_archive(bytes: &[u8], compression: &str) -> Vec<u8> {
    match compression {
        "zstd" => zstd::stream::decode_all(Cursor::new(bytes)).expect("decode zstd archive"),
        _ => bytes.to_vec(),
    }
}

pub fn read_archive_paths(bytes: &[u8], compression: &str) -> Vec<String> {
    let decoded = decode_archive(bytes, compression);
    let mut archive = tar::Archive::new(Cursor::new(decoded));
    archive
        .entries()
        .expect("archive entries")
        .map(|entry| {
            entry
                .expect("archive entry")
                .path()
                .expect("entry path")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

pub fn read_single_file_archive(bytes: &[u8]) -> (String, Vec<u8>) {
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut entries = archive.entries().expect("archive entries");
    let mut entry = entries
        .next()
        .expect("archive entry")
        .expect("archive entry ok");
    let path = entry
        .path()
        .expect("entry path")
        .to_string_lossy()
        .into_owned();
    let mut body = Vec::new();
    entry.read_to_end(&mut body).expect("entry body");
    assert!(
        entries
            .next()
            .transpose()
            .expect("no extra entries")
            .is_none(),
        "single-file archive contained extra entries"
    );
    (path, body)
}

pub fn raw_tar_file_with_path(path: impl AsRef<Path>, body: &[u8]) -> Vec<u8> {
    fn write_octal(field: &mut [u8], value: u64) {
        let digits = field.len() - 1;
        let text = format!("{value:o}");
        assert!(
            text.len() <= digits,
            "value {value} does not fit in tar field"
        );
        let start = digits - text.len();
        field[..start].fill(b'0');
        field[start..digits].copy_from_slice(text.as_bytes());
        field[digits] = 0;
    }

    fn write_checksum(field: &mut [u8], checksum: u32) {
        let text = format!("{checksum:o}");
        assert!(
            text.len() <= 6,
            "checksum {checksum} does not fit in tar field"
        );
        let start = 6 - text.len();
        field[..start].fill(b'0');
        field[start..6].copy_from_slice(text.as_bytes());
        field[6] = 0;
        field[7] = b' ';
    }

    let path = path.as_ref().to_string_lossy();
    assert!(
        path.len() <= 100,
        "tar test helper only supports short paths"
    );
    let mut header = [0u8; 512];
    header[..path.len()].copy_from_slice(path.as_bytes());
    write_octal(&mut header[100..108], 0o644);
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], body.len() as u64);
    write_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| *byte as u32).sum();
    write_checksum(&mut header[148..156], checksum);

    let mut archive = Vec::with_capacity(512 + body.len() + 1024);
    archive.extend_from_slice(&header);
    archive.extend_from_slice(body);
    let padding = (512 - (body.len() % 512)) % 512;
    archive.resize(archive.len() + padding, 0);
    archive.extend_from_slice(&[0u8; 1024]);
    archive
}

pub fn multi_source_tar() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());

    let file_body = b"alpha\n";
    let mut alpha = tar::Header::new_gnu();
    alpha.set_entry_type(tar::EntryType::Regular);
    alpha.set_mode(0o644);
    alpha.set_size(file_body.len() as u64);
    alpha.set_cksum();
    builder
        .append_data(
            &mut alpha,
            "alpha.txt",
            std::io::Cursor::new(file_body.as_slice()),
        )
        .unwrap();

    let mut nested = tar::Header::new_gnu();
    nested.set_entry_type(tar::EntryType::Directory);
    nested.set_mode(0o755);
    nested.set_size(0);
    nested.set_cksum();
    builder
        .append_data(&mut nested, "nested", std::io::empty())
        .unwrap();

    let nested_body = b"beta\n";
    let mut beta = tar::Header::new_gnu();
    beta.set_entry_type(tar::EntryType::Regular);
    beta.set_mode(0o644);
    beta.set_size(nested_body.len() as u64);
    beta.set_cksum();
    builder
        .append_data(
            &mut beta,
            "nested/beta.txt",
            std::io::Cursor::new(nested_body.as_slice()),
        )
        .unwrap();

    builder.finish().unwrap();
    builder.into_inner().unwrap()
}

#[cfg(unix)]
pub fn directory_tar_with_symlink() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());

    let file_body = b"alpha\n";
    let mut alpha = tar::Header::new_gnu();
    alpha.set_entry_type(tar::EntryType::Regular);
    alpha.set_mode(0o644);
    alpha.set_size(file_body.len() as u64);
    alpha.set_cksum();
    builder
        .append_data(
            &mut alpha,
            "alpha.txt",
            std::io::Cursor::new(file_body.as_slice()),
        )
        .unwrap();

    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    builder
        .append_link(&mut link, "alpha-link", "alpha.txt")
        .unwrap();

    builder.finish().unwrap();
    builder.into_inner().unwrap()
}
