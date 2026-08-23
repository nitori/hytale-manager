//! The subset of BSON the credential document uses.
//!
//! The server encodes `auth.enc`'s plaintext with its own `BuilderCodec`, which writes a
//! BSON document; every field in that document is a string. Encoding that by hand is some
//! forty lines, against a dependency that would pull serde, uuid, and a date library to
//! represent four strings.

use std::collections::BTreeMap;

use crate::error::{Error, Result};

/// BSON's type byte for a UTF-8 string.
const STRING: u8 = 0x02;

/// Encode `fields` as a BSON document, in the order given.
pub fn encode(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.push(STRING);
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        // The length prefix counts the terminator, unlike the document's own.
        body.extend_from_slice(&(value.len() as i32 + 1).to_le_bytes());
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }

    let total = body.len() as i32 + 5;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&body);
    out.push(0);
    out
}

/// Read a BSON document of string fields. Fields of other types are skipped rather than
/// refused: a future server version adding one must not make credentials unreadable.
pub fn decode(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let total = read_i32(bytes, 0)? as usize;
    if total != bytes.len() || total < 5 {
        return Err(Error::Corrupt("document length does not match the payload"));
    }

    let mut fields = BTreeMap::new();
    let mut at = 4;
    loop {
        let kind = *bytes.get(at).ok_or(Error::Corrupt("truncated document"))?;
        if kind == 0 {
            return Ok(fields);
        }
        at += 1;

        let name_end = bytes[at..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(Error::Corrupt("unterminated field name"))?
            + at;
        let name = std::str::from_utf8(&bytes[at..name_end])
            .map_err(|_| Error::Corrupt("field name is not UTF-8"))?
            .to_owned();
        at = name_end + 1;

        if kind != STRING {
            return Err(Error::Corrupt("expected a document of strings"));
        }

        let len = read_i32(bytes, at)? as usize;
        at += 4;
        let value = bytes
            .get(at..at + len.saturating_sub(1))
            .ok_or(Error::Corrupt("truncated string"))?;
        fields.insert(
            name,
            std::str::from_utf8(value)
                .map_err(|_| Error::Corrupt("field value is not UTF-8"))?
                .to_owned(),
        );
        at += len;
    }
}

fn read_i32(bytes: &[u8], at: usize) -> Result<i32> {
    let slice = bytes
        .get(at..at + 4)
        .ok_or(Error::Corrupt("truncated length prefix"))?;
    Ok(i32::from_le_bytes(slice.try_into().expect("four bytes")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let encoded = encode(&[("AccessToken", "abc"), ("ProfileUuid", "")]);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded["AccessToken"], "abc");
        assert_eq!(decoded["ProfileUuid"], "");
    }

    /// Checked against the format rather than our own encoder, since agreeing with
    /// ourselves proves nothing about agreeing with the server.
    #[test]
    fn matches_the_bson_spec_byte_for_byte() {
        // { "a": "b" } is 0x0e bytes: len, 0x02, "a\0", len(2), "b\0", terminator.
        assert_eq!(
            encode(&[("a", "b")]),
            vec![0x0e, 0, 0, 0, 0x02, b'a', 0, 0x02, 0, 0, 0, b'b', 0, 0]
        );
    }

    #[test]
    fn a_truncated_document_is_an_error() {
        let mut encoded = encode(&[("a", "b")]);
        encoded.pop();
        assert!(decode(&encoded).is_err());
    }

    #[test]
    fn a_wrong_length_prefix_is_an_error() {
        let mut encoded = encode(&[("a", "b")]);
        encoded[0] = 0x7f;
        assert!(decode(&encoded).is_err());
    }
}
