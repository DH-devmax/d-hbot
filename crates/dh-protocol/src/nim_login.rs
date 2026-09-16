//! Wire construction only. Caller supplies verified SDK-equivalent field values.
//! No endpoint discovery, credential acquisition or successful authentication implied.
use crate::{nim_header::Header, nim_property};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    UnknownField,
    DuplicateField,
    MissingIdentity,
    Encoding(nim_property::Error),
    MissingConnection,
    InvalidResponse,
}
/// Full SDK auth response: Property, PropertyArray, Property. Unknown fields
/// are retained, but truncation and unconsumed trailing bytes are rejected.
pub struct AuthResponse {
    pub login: LoginResponse,
    pub ports: Vec<nim_property::Property>,
    pub push: nim_property::Property,
}
pub fn decode_auth(bytes: &[u8]) -> Result<AuthResponse, Error> {
    const LIMIT: usize = 1024 * 1024;
    if bytes.len() > LIMIT {
        return Err(Error::InvalidResponse);
    }
    let (fields, mut offset) = nim_property::decode(bytes, 128, LIMIT).map_err(Error::Encoding)?;
    if fields.iter().filter(|(id, _)| *id == 102).count() != 1 {
        return Err(Error::InvalidResponse);
    }
    let login = decode_response(fields);
    require_connection(&login)?;
    if offset == bytes.len() {
        return Ok(AuthResponse {
            login,
            ports: Vec::new(),
            push: Vec::new(),
        });
    }
    let mut count = 0usize;
    for shift in (0..35).step_by(7) {
        let b = *bytes.get(offset).ok_or(Error::InvalidResponse)?;
        offset += 1;
        count |= ((b & 127) as usize) << shift;
        if b < 128 {
            break;
        }
        if shift == 28 {
            return Err(Error::InvalidResponse);
        }
    }
    if count > 128 {
        return Err(Error::InvalidResponse);
    }
    let mut ports = Vec::with_capacity(count);
    for _ in 0..count {
        let (port, used) =
            nim_property::decode(&bytes[offset..], 128, LIMIT).map_err(Error::Encoding)?;
        offset += used;
        ports.push(port);
    }
    let (push, used) = if offset == bytes.len() {
        (Vec::new(), 0)
    } else {
        nim_property::decode(&bytes[offset..], 128, LIMIT).map_err(Error::Encoding)?
    };
    if offset + used != bytes.len() {
        return Err(Error::InvalidResponse);
    }
    Ok(AuthResponse { login, ports, push })
}
/// Successful auth/2_3 responses contain loginRes first, then loginPorts and push info.
/// This helper only extracts the verified numeric loginRes fields and preserves the rest.
#[derive(Default, PartialEq, Eq)]
pub struct LoginResponse {
    pub connection_id: Option<Vec<u8>>,
    pub ip: Option<Vec<u8>>,
    pub port: Option<Vec<u8>>,
    pub raw_login_res: nim_property::Property,
}
pub fn decode_response(fields: nim_property::Property) -> LoginResponse {
    let mut out = LoginResponse::default();
    for (id, value) in fields {
        match id {
            102 => out.connection_id = Some(value.clone()),
            103 => out.ip = Some(value.clone()),
            104 => out.port = Some(value.clone()),
            _ => {}
        }
        out.raw_login_res.push((id, value));
    }
    out
}
pub fn require_connection(response: &LoginResponse) -> Result<&[u8], Error> {
    response
        .connection_id
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or(Error::MissingConnection)
}
/// IDs recovered from the reviewed 2.8.0 SDK login entity.
fn field_id(name: &str) -> Option<u32> {
    Some(match name {
        "clientType" => 3,
        "os" => 4,
        "sdkVersion" => 6,
        "appLogin" => 8,
        "protocolVersion" => 9,
        "pushTokenName" => 10,
        "pushToken" => 11,
        "deviceId" => 13,
        "appKey" => 18,
        "account" => 19,
        "browser" => 24,
        "session" => 26,
        "deviceInfo" => 32,
        "customTag" => 38,
        "customClientType" => 39,
        "sdkHumanVersion" => 40,
        "sdkType" => 41,
        "userAgent" => 42,
        "libEnv" => 44,
        "isReactNative" => 112,
        "authType" => 115,
        "loginExt" => 116,
        "token" => 1000,
        _ => return None,
    })
}
/// Ordered numeric properties match JavaScript integer-key enumeration.
/// All values must already have their SDK string/JSON representation.
pub fn encode(serial: u16, fields: &[(&str, &str)]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut ordered = BTreeMap::new();
    for (name, value) in fields {
        let id = field_id(name).ok_or(Error::UnknownField)?;
        if ordered.insert(id, *value).is_some() {
            return Err(Error::DuplicateField);
        }
    }
    for id in [13, 18, 19, 26, 1000] {
        if ordered.get(&id).is_none_or(|s| s.is_empty()) {
            return Err(Error::MissingIdentity);
        }
    }
    let mut properties: Vec<_> = ordered
        .into_iter()
        .map(|(k, v)| (k, v.as_bytes().to_vec()))
        .collect();
    let body = nim_property::encode(&properties, 25, 64 * 1024);
    use zeroize::Zeroize;
    for (_, value) in &mut properties {
        value.zeroize();
    }
    let body = Zeroizing::new(body.map_err(Error::Encoding)?);
    let mut packet = Zeroizing::new(Header::request(2, 3, serial).to_vec());
    packet.extend_from_slice(&body);
    Ok(packet)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_auth_tails_and_truncation() {
        let login = nim_property::encode(&[(102, b"connection".to_vec())], 128, 1024).unwrap();
        for tail in [vec![], vec![0], vec![0, 0]] {
            let mut packet = login.clone();
            packet.extend(tail);
            let response = decode_auth(&packet).unwrap();
            assert!(response.ports.is_empty());
            assert!(response.push.is_empty());
        }
        for tail in [vec![128], vec![1], vec![0, 1], vec![0, 0, 0]] {
            let mut packet = login.clone();
            packet.extend(tail);
            assert!(decode_auth(&packet).is_err());
        }
        let duplicate =
            nim_property::encode(&[(102, b"a".to_vec()), (102, b"b".to_vec())], 128, 1024).unwrap();
        assert!(decode_auth(&duplicate).is_err());
    }
    #[test]
    fn sorted_fields_and_no_credential_debug() {
        let fields = [
            ("token", "t"),
            ("session", "s"),
            ("account", "a"),
            ("appKey", "k"),
            ("deviceId", "d"),
        ];
        let packet = encode(0x1234, &fields).unwrap();
        assert_eq!(&packet[..6], &[0, 2, 3, 52, 18, 0]);
        let (properties, consumed) = nim_property::decode(&packet[6..], 25, 65536).unwrap();
        assert_eq!(consumed, packet.len() - 6);
        assert_eq!(
            properties.iter().map(|p| p.0).collect::<Vec<_>>(),
            [13, 18, 19, 26, 1000]
        );
        let response = decode_response(vec![(102, b"connection".to_vec()), (999, vec![1])]);
        assert_eq!(require_connection(&response), Ok(&b"connection"[..]));
        assert_eq!(
            require_connection(&decode_response(vec![])),
            Err(Error::MissingConnection)
        );
        assert_eq!(encode(0, &[("guessed", "x")]), Err(Error::UnknownField));
        assert_eq!(encode(0, &[]), Err(Error::MissingIdentity));
        assert_eq!(
            encode(0, &[("token", "a"), ("token", "b")]),
            Err(Error::DuplicateField)
        );
    }
}
