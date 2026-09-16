//! NIM Property: varint count, then varint field ID and varint-length raw bytes.
//! Values remain bytes: unknown fields and non-UTF8 data are not discarded.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    Overflow,
    Limit,
}
pub type Property = Vec<(u32, Vec<u8>)>;
fn put_varint(mut value: u32, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}
/// Encodes ordered wire properties without interpreting or logging values.
/// Caller must supply SDK field ordering for newly constructed requests.
pub fn encode(
    fields: &[(u32, Vec<u8>)],
    max_fields: usize,
    max_bytes: usize,
) -> Result<Vec<u8>, Error> {
    if fields.len() > max_fields {
        return Err(Error::Limit);
    }
    let count = u32::try_from(fields.len()).map_err(|_| Error::Overflow)?;
    let mut out = Vec::new();
    put_varint(count, &mut out);
    if out.len() > max_bytes {
        return Err(Error::Limit);
    }
    for (id, value) in fields {
        let length = u32::try_from(value.len()).map_err(|_| Error::Overflow)?;
        let mut prefix = Vec::new();
        put_varint(*id, &mut prefix);
        put_varint(length, &mut prefix);
        let end = out
            .len()
            .checked_add(prefix.len())
            .and_then(|n| n.checked_add(value.len()))
            .ok_or(Error::Overflow)?;
        if end > max_bytes {
            return Err(Error::Limit);
        }
        out.extend_from_slice(&prefix);
        out.extend_from_slice(value);
    }
    Ok(out)
}
fn varint(input: &[u8], pos: &mut usize) -> Result<u32, Error> {
    let mut value = 0;
    for shift in (0..35).step_by(7) {
        let byte = *input.get(*pos).ok_or(Error::Truncated)?;
        *pos += 1;
        if shift == 28 && byte > 15 {
            return Err(Error::Overflow);
        }
        value |= u32::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(Error::Overflow)
}
pub fn decode(
    input: &[u8],
    max_fields: usize,
    max_bytes: usize,
) -> Result<(Property, usize), Error> {
    if input.len() > max_bytes {
        return Err(Error::Limit);
    }
    let mut pos = 0;
    let count = varint(input, &mut pos)? as usize;
    if count > max_fields {
        return Err(Error::Limit);
    }
    let mut fields = Vec::new();
    for _ in 0..count {
        let id = varint(input, &mut pos)?;
        let size = varint(input, &mut pos)? as usize;
        let end = pos.checked_add(size).ok_or(Error::Overflow)?;
        let value = input.get(pos..end).ok_or(Error::Truncated)?;
        fields.push((id, value.to_vec()));
        pos = end;
    }
    Ok((fields, pos))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn property_encoding_and_limits() {
        let fields = vec![(19, b"demo".to_vec()), (1000, b"synthetic".to_vec())];
        let expected = [
            2, 19, 4, 100, 101, 109, 111, 232, 7, 9, 115, 121, 110, 116, 104, 101, 116, 105, 99,
        ];
        assert_eq!(encode(&fields, 2, 19), Ok(expected.to_vec()));
        assert_eq!(decode(&expected, 2, 19), Ok((fields.clone(), 19)));
        assert_eq!(encode(&fields, 2, 18), Err(Error::Limit));
        assert_eq!(encode(&fields, 1, 19), Err(Error::Limit));
        assert_eq!(encode(&[], 0, 0), Err(Error::Limit));
    }
    #[test]
    fn preserves_unknown_duplicate_and_binary_fields() {
        let bytes = [3, 19, 1, b'a', 0xe8, 7, 2, 0xff, 0, 19, 0, 99];
        assert_eq!(
            decode(&bytes, 3, 64),
            Ok((
                vec![(19, vec![b'a']), (1000, vec![255, 0]), (19, vec![])],
                11
            ))
        );
        for n in 0..11 {
            assert!(decode(&bytes[..n], 3, 64).is_err());
        }
        assert_eq!(decode(&bytes, 2, 64), Err(Error::Limit));
        assert_eq!(decode(&bytes, 3, 4), Err(Error::Limit));
    }
    #[test]
    fn rejects_overflow_before_allocation() {
        assert_eq!(
            decode(&[255, 255, 255, 255, 16], usize::MAX, 64),
            Err(Error::Overflow)
        );
    }
}
