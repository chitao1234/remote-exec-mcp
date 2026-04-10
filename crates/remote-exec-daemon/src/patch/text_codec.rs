use std::borrow::Cow;
use std::path::Path;

use chardetng::EncodingDetector;
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE};

#[derive(Debug, Clone)]
pub(crate) struct PatchTextFile {
    pub(crate) text: String,
    encoding: PatchTextEncoding,
}

#[derive(Debug, Clone)]
enum PatchTextEncoding {
    Utf8,
    Detected {
        encoding: &'static Encoding,
        bom: Vec<u8>,
    },
}

impl PatchTextFile {
    pub(crate) async fn read(path: &Path, allow_encoding_autodetect: bool) -> anyhow::Result<Self> {
        let bytes = tokio::fs::read(path).await?;
        Self::from_bytes(bytes, allow_encoding_autodetect)
    }

    pub(crate) fn encode(&self, text: &str) -> anyhow::Result<Vec<u8>> {
        match &self.encoding {
            PatchTextEncoding::Utf8 => Ok(text.as_bytes().to_vec()),
            PatchTextEncoding::Detected { encoding, bom } => {
                let encoded = encode_text_for_encoding(encoding, text)?;
                let mut output = Vec::with_capacity(bom.len() + encoded.len());
                output.extend_from_slice(bom);
                output.extend_from_slice(&encoded);
                Ok(output)
            }
        }
    }

    fn from_bytes(bytes: Vec<u8>, allow_encoding_autodetect: bool) -> anyhow::Result<Self> {
        match String::from_utf8(bytes) {
            Ok(text) => Ok(Self {
                text,
                encoding: PatchTextEncoding::Utf8,
            }),
            Err(err) if allow_encoding_autodetect => {
                let bytes = err.into_bytes();
                let (encoding, bom_len) = detect_encoding(&bytes);
                let content_bytes = &bytes[bom_len..];
                let text = decode_without_replacement(encoding, content_bytes)?.into_owned();

                anyhow::ensure!(
                    looks_like_text(&text),
                    "detected {} content does not look like text",
                    encoding.name()
                );

                anyhow::ensure!(
                    encode_text_for_encoding(encoding, &text)?.as_slice() == content_bytes,
                    "detected {} content did not round-trip cleanly",
                    encoding.name()
                );

                Ok(Self {
                    text,
                    encoding: PatchTextEncoding::Detected {
                        encoding,
                        bom: bytes[..bom_len].to_vec(),
                    },
                })
            }
            Err(err) => Err(err.into()),
        }
    }
}

fn detect_encoding(bytes: &[u8]) -> (&'static Encoding, usize) {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        return (encoding, bom_len);
    }

    let mut detector = EncodingDetector::new();
    detector.feed(bytes, true);
    (detector.guess(None, true), 0)
}

fn decode_without_replacement<'a>(
    encoding: &'static Encoding,
    bytes: &'a [u8],
) -> anyhow::Result<Cow<'a, str>> {
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .ok_or_else(|| anyhow::anyhow!("target file is not valid {} text", encoding.name()))
}

fn looks_like_text(text: &str) -> bool {
    if text.contains('\0') {
        return false;
    }

    let suspicious_controls = text
        .chars()
        .filter(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t' | '\u{000C}'))
        .count();
    let char_count = text.chars().count();

    suspicious_controls == 0 || suspicious_controls * 20 <= char_count.max(1)
}

fn encode_text_for_encoding(encoding: &'static Encoding, text: &str) -> anyhow::Result<Vec<u8>> {
    // encoding_rs handles the WHATWG legacy text encodings we care about
    // directly. UTF-16 is the exception: its Rust encode API intentionally
    // emits UTF-8 bytes, so preserve UTF-16LE/BE manually here.
    if encoding == UTF_16LE {
        return Ok(text
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect());
    }

    if encoding == UTF_16BE {
        return Ok(text
            .encode_utf16()
            .flat_map(|unit| unit.to_be_bytes())
            .collect());
    }

    let (encoded, _, had_errors) = encoding.encode(text);
    anyhow::ensure!(
        !had_errors,
        "updated text cannot be represented in detected {} encoding",
        encoding.name()
    );
    Ok(encoded.into_owned())
}

#[cfg(test)]
mod tests {
    use encoding_rs::{BIG5, EUC_KR, GBK, SHIFT_JIS};

    use super::{PatchTextFile, encode_text_for_encoding};

    #[test]
    fn autodetect_decodes_utf16le_with_bom_and_preserves_encoding() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend([b'h', 0, b'i', 0, b'\n', 0]);

        let file = PatchTextFile::from_bytes(bytes.clone(), true).unwrap();

        assert_eq!(file.text, "hi\n");
        assert_eq!(file.encode("bye\n").unwrap(), {
            let mut expected = vec![0xFF, 0xFE];
            expected.extend([b'b', 0, b'y', 0, b'e', 0, b'\n', 0]);
            expected
        });
        assert_ne!(file.encode("bye\n").unwrap(), "bye\n".as_bytes());
    }

    #[test]
    fn autodetect_rejects_invalid_utf8_when_disabled() {
        let err = PatchTextFile::from_bytes(vec![0xFF, 0xFE, 0xFD], false).unwrap_err();
        assert!(err.to_string().contains("invalid utf-8"));
    }

    #[test]
    fn autodetect_rejects_likely_binary_content() {
        let err =
            PatchTextFile::from_bytes(vec![0xFF, 0x00, 0xFE, 0x01, 0xFD, 0x02], true).unwrap_err();
        assert!(err.to_string().contains("does not look like text"));
    }

    #[test]
    fn autodetect_round_trips_common_east_asian_encodings() {
        let cases = [
            (
                SHIFT_JIS,
                "価格を更新します。\n次の行です。\n",
                "価格を反映します。\n次の行です。\n",
            ),
            (
                GBK,
                "简体中文文件。\n第二行内容。\n",
                "简体中文配置。\n第二行内容。\n",
            ),
            (
                BIG5,
                "繁體中文檔案。\n第二行內容。\n",
                "繁體中文設定。\n第二行內容。\n",
            ),
            (
                EUC_KR,
                "한국어 파일입니다.\n둘째 줄입니다.\n",
                "한국어 설정입니다.\n둘째 줄입니다.\n",
            ),
        ];

        for (encoding, original_text, updated_text) in cases {
            let original_bytes = encode_text_for_encoding(encoding, original_text).unwrap();
            let file = PatchTextFile::from_bytes(original_bytes, true).unwrap();

            assert_eq!(
                file.text,
                original_text,
                "failed to decode {}",
                encoding.name()
            );
            assert_eq!(
                file.encode(updated_text).unwrap(),
                encode_text_for_encoding(encoding, updated_text).unwrap(),
                "failed to preserve {}",
                encoding.name()
            );
        }
    }
}
