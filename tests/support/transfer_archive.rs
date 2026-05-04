use std::io::Cursor;

pub fn decode_archive(bytes: &[u8], compression: &str) -> Vec<u8> {
    match compression {
        "zstd" => zstd::stream::decode_all(Cursor::new(bytes)).expect("decode zstd archive"),
        _ => bytes.to_vec(),
    }
}

#[allow(
    dead_code,
    reason = "This shared test helper is compiled independently in broker and daemon test crates"
)]
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
