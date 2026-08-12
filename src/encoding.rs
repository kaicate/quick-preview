use encoding_rs::SHIFT_JIS;

use crate::{PreviewError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    ShiftJis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingInfo {
    pub encoding: TextEncoding,
    pub bom: bool,
}

pub fn detect(bytes: &[u8]) -> EncodingInfo {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return EncodingInfo {
            encoding: TextEncoding::Utf8,
            bom: true,
        };
    }
    EncodingInfo {
        encoding: if std::str::from_utf8(bytes).is_ok() {
            TextEncoding::Utf8
        } else {
            TextEncoding::ShiftJis
        },
        bom: false,
    }
}

pub fn payload<'a>(bytes: &'a [u8], info: EncodingInfo) -> &'a [u8] {
    if info.bom && bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    }
}

pub fn decode(bytes: &[u8], encoding: TextEncoding) -> String {
    match encoding {
        TextEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        TextEncoding::ShiftJis => SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

pub fn encode(text: &str, encoding: TextEncoding) -> Result<Vec<u8>> {
    match encoding {
        TextEncoding::Utf8 => Ok(text.as_bytes().to_vec()),
        TextEncoding::ShiftJis => {
            let (encoded, _, had_errors) = SHIFT_JIS.encode(text);
            if had_errors {
                Err(PreviewError::UnrepresentableShiftJis)
            } else {
                Ok(encoded.into_owned())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bom_and_shift_jis() {
        assert_eq!(
            detect(b"\xef\xbb\xbfhello"),
            EncodingInfo {
                encoding: TextEncoding::Utf8,
                bom: true
            }
        );
        assert_eq!(
            detect(&[0x93, 0xfa, 0x96, 0x7b]),
            EncodingInfo {
                encoding: TextEncoding::ShiftJis,
                bom: false
            }
        );
    }

    #[test]
    fn shift_jis_round_trip() {
        let bytes = encode("日本語", TextEncoding::ShiftJis).unwrap();
        assert_eq!(decode(&bytes, TextEncoding::ShiftJis), "日本語");
        assert!(encode("😀", TextEncoding::ShiftJis).is_err());
    }
}
