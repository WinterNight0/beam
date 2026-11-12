//! Lowercase hex, shared by fingerprints, transfer IDs and chunk hashes.

/// Why a string could not be read as hex.
#[derive(Debug, thiserror::Error)]
pub enum HexError {
    #[error("expected {expected} hex characters, got {actual}")]
    Length { expected: usize, actual: usize },
    #[error("not valid hex")]
    NotHex,
}

/// Encodes bytes as lowercase hex.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(digit(byte >> 4));
        out.push(digit(byte & 0x0f));
    }
    out
}

/// Decodes hex into a buffer whose length fixes how many characters are
/// expected. Case is ignored.
pub fn decode_into(text: &str, out: &mut [u8]) -> Result<(), HexError> {
    let expected = out.len() * 2;
    if text.len() != expected {
        return Err(HexError::Length {
            expected,
            actual: text.len(),
        });
    }
    let bytes = text.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (value(bytes[i * 2])? << 4) | value(bytes[i * 2 + 1])?;
    }
    Ok(())
}

fn digit(nibble: u8) -> char {
    char::from(match nibble {
        0..=9 => b'0' + nibble,
        _ => b'a' + nibble - 10,
    })
}

fn value(c: u8) -> Result<u8, HexError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(HexError::NotHex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let bytes = [0x00, 0x0f, 0x10, 0xff, 0xa5];
        let text = encode(&bytes);
        assert_eq!(text, "000f10ffa5");

        let mut back = [0u8; 5];
        decode_into(&text, &mut back).expect("decode");
        assert_eq!(back, bytes);
    }

    #[test]
    fn decode_ignores_case() {
        let mut out = [0u8; 2];
        decode_into("AbCd", &mut out).expect("decode");
        assert_eq!(out, [0xab, 0xcd]);
    }

    #[test]
    fn decode_rejects_bad_input() {
        let mut out = [0u8; 2];
        assert!(matches!(
            decode_into("abc", &mut out),
            Err(HexError::Length { .. })
        ));
        assert!(matches!(
            decode_into("zzzz", &mut out),
            Err(HexError::NotHex)
        ));
    }
}
