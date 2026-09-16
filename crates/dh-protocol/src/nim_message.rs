//! Reviewed SDK message properties and receipt routes (SDK 9.21.14).
//! Custom content is still an application envelope, never assumed to be text.
use crate::{nim_header::Header, nim_property, nim_transport::Packet};
use std::collections::BTreeMap;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_MESSAGES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Limit,
    Unsupported,
    Rejected(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    Private,
    Group,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Message {
    pub scene: Scene,
    pub to: String,
    pub from: String,
    pub nickname: String,
    pub time: u64,
    pub kind: u32,
    pub body: String,
    pub attachment: String,
    pub client_id: String,
    pub server_id: String,
}

// Do not put message content or participant identities into diagnostics.
impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Message")
            .field("scene", &self.scene)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Message {
    fn parse(fields: nim_property::Property) -> Result<Self, Error> {
        let mut values = BTreeMap::new();
        for (id, value) in fields {
            if values.insert(id, value).is_some() {
                return Err(Error::Malformed);
            }
        }
        let text = |id| -> Result<String, Error> {
            String::from_utf8(values.get(&id).cloned().unwrap_or_default())
                .map_err(|_| Error::Malformed)
        };
        let scene = match text(0)?.as_str() {
            "0" => Scene::Private,
            "1" => Scene::Group,
            _ => return Err(Error::Unsupported),
        };
        let result = Self {
            scene,
            to: text(1)?,
            from: text(2)?,
            nickname: text(6)?,
            time: text(7)?.parse().map_err(|_| Error::Malformed)?,
            kind: text(8)?.parse().map_err(|_| Error::Malformed)?,
            body: text(9)?,
            attachment: text(10)?,
            client_id: text(11)?,
            server_id: text(12)?,
        };
        if result.to.is_empty() || result.from.is_empty() || result.server_id.is_empty() {
            return Err(Error::Malformed);
        }
        Ok(result)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Messages(Vec<Message>),
    Recalled { target: String, server_id: String },
    SyncDone(u64),
    Unhandled { service: u8, command: u8 },
}

fn varint(input: &[u8], offset: &mut usize) -> Result<usize, Error> {
    let mut n = 0u32;
    for shift in (0..35).step_by(7) {
        let b = *input.get(*offset).ok_or(Error::Malformed)?;
        *offset += 1;
        if shift == 28 && b > 15 {
            return Err(Error::Malformed);
        }
        n |= u32::from(b & 127) << shift;
        if b < 128 {
            return Ok(n as usize);
        }
    }
    Err(Error::Malformed)
}
fn put_varint(mut value: usize, bytes: &mut Vec<u8>) {
    while value >= 128 {
        bytes.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
}

pub fn decode(packet: &Packet) -> Result<Event, Error> {
    if packet.body.len() > MAX_BYTES {
        return Err(Error::Limit);
    }
    if packet.header.response_code != 200 {
        return Err(Error::Rejected(packet.header.response_code));
    }
    let route = (packet.header.service, packet.header.command);
    // SDK notify packets contain an 8-byte notification ID and a complete inner header.
    if matches!(route, (4, 1 | 2 | 10 | 11)) {
        let inner = crate::nim_transport::decode_message(
            packet.body.get(8..).ok_or(Error::Malformed)?,
            MAX_BYTES,
        )
        .map_err(|_| Error::Malformed)?;
        if inner.header.service == 4 {
            return Err(Error::Malformed);
        }
        return decode(&inner);
    }

    if route == (7, 14) {
        let (fields, used) =
            nim_property::decode(&packet.body, 128, MAX_BYTES).map_err(|_| Error::Malformed)?;
        if used != packet.body.len() {
            return Err(Error::Malformed);
        }
        let mut data = BTreeMap::new();
        for (key, value) in fields {
            if data.insert(key, value).is_some() {
                return Err(Error::Malformed);
            }
        }
        if data.get(&1).map(Vec::as_slice) != Some(b"8".as_slice()) {
            return Err(Error::Unsupported);
        }
        let target = String::from_utf8(data.remove(&2).ok_or(Error::Malformed)?)
            .map_err(|_| Error::Malformed)?;
        let server_id = String::from_utf8(data.remove(&11).ok_or(Error::Malformed)?)
            .map_err(|_| Error::Malformed)?;
        if target.is_empty() || server_id.parse::<u64>().unwrap_or(0) == 0 {
            return Err(Error::Malformed);
        }
        return Ok(Event::Recalled { target, server_id });
    }
    if route == (5, 1) {
        return Ok(Event::SyncDone(u64::from_le_bytes(
            packet
                .body
                .as_slice()
                .try_into()
                .map_err(|_| Error::Malformed)?,
        )));
    }
    let mut offset = 0;
    let count = match route {
        (7, 2 | 101) | (8, 3 | 102) => 1,
        (4, 4 | 9) | (8, 4) => varint(&packet.body, &mut offset)?,
        _ => {
            return Ok(Event::Unhandled {
                service: route.0,
                command: route.1,
            })
        }
    };
    if count > MAX_MESSAGES {
        return Err(Error::Limit);
    }
    let mut messages = Vec::with_capacity(count);
    for _ in 0..count {
        let (fields, used) = nim_property::decode(&packet.body[offset..], 128, MAX_BYTES)
            .map_err(|_| Error::Malformed)?;
        offset += used;
        messages.push(Message::parse(fields)?);
    }
    if offset != packet.body.len() {
        return Err(Error::Malformed);
    }
    Ok(Event::Messages(messages))
}

/// NIM custom messages carry the application-encrypted content in property 10.
pub fn custom_request(
    serial: u16,
    scene: Scene,
    to: &str,
    client_id: &str,
    content: &str,
) -> Result<Vec<u8>, Error> {
    if to.is_empty() || client_id.is_empty() || content.is_empty() {
        return Err(Error::Malformed);
    }
    let fields = [
        (0, if scene == Scene::Group { "1" } else { "0" }),
        (1, to),
        (8, "100"),
        (10, content),
        (11, client_id),
    ];
    let fields = fields
        .into_iter()
        .map(|(id, value)| (id, value.as_bytes().to_vec()))
        .collect::<Vec<_>>();
    let body = nim_property::encode(&fields, 128, MAX_BYTES).map_err(|_| Error::Limit)?;
    let (service, command) = send_route(scene);
    let mut frame = Header::request(service, command, serial).to_vec();
    frame.extend(body);
    Ok(frame)
}

/// This is a transport acknowledgement, not a user read receipt. Persist first.
pub fn acknowledge(serial: u16, scene: Scene, server_ids: &[String]) -> Result<Vec<u8>, Error> {
    if server_ids.is_empty() || server_ids.len() > MAX_MESSAGES {
        return Err(Error::Limit);
    }
    let mut frame = Header::request(4, 5, serial).to_vec();
    frame.extend(if scene == Scene::Group {
        [8, 3]
    } else {
        [7, 2]
    });
    put_varint(server_ids.len(), &mut frame);
    for id in server_ids {
        let id = id.parse::<u64>().map_err(|_| Error::Malformed)?;
        frame.extend(id.to_le_bytes());
    }
    Ok(frame)
}

/// SDK sync fields 2/7 request offline and roaming messages from the last
/// fully persisted sync boundary. Other roster/settings sync stays on HTTP.
pub fn sync_request(serial: u16, cursor: u64) -> Vec<u8> {
    let value = cursor.to_string().into_bytes();
    let body =
        nim_property::encode(&[(2, value.clone()), (7, value)], 2, 128).expect("bounded cursor");
    let mut frame = Header::request(5, 1, serial).to_vec();
    frame.extend(body);
    frame
}

pub fn send_route(scene: Scene) -> (u8, u8) {
    match scene {
        Scene::Group => (8, 2),
        Scene::Private => (7, 1),
    }
}

/// Reviewed SDK recallMsg uses msg.deleteMsg (7/13), with sysMsg type 8 for groups.
pub fn recall_request(
    serial: u16,
    target: &str,
    from: &str,
    client: &str,
    server: &str,
    time: u64,
) -> Result<Vec<u8>, Error> {
    if target.is_empty()
        || from.is_empty()
        || client.is_empty()
        || time == 0
        || server.parse::<u64>().unwrap_or(0) == 0
    {
        return Err(Error::Malformed);
    }
    let time = time.to_string();
    let fields = [
        (0, time.as_str()),
        (1, "8"),
        (2, target),
        (3, from),
        (10, client),
        (11, server),
        (16, from),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.as_bytes().to_vec()))
    .collect::<Vec<_>>();
    let mut frame = Header::request(7, 13, serial).to_vec();
    frame.extend(nim_property::encode(&fields, 128, MAX_BYTES).map_err(|_| Error::Limit)?);
    Ok(frame)
}

pub fn member_mute_request(
    serial: u16,
    team: u64,
    account: &str,
    muted: bool,
) -> Result<Vec<u8>, Error> {
    if team == 0 || account.is_empty() || account.len() > 1024 {
        return Err(Error::Malformed);
    }
    let mut frame = Header::request(8, 25, serial).to_vec();
    frame.extend(team.to_le_bytes());
    put_varint(account.len(), &mut frame);
    frame.extend(account.as_bytes());
    frame.extend(u32::from(muted).to_le_bytes());
    Ok(frame)
}

pub fn muted_member_present(body: &[u8], team: u64, account: &str) -> Result<bool, Error> {
    if body.len() > MAX_BYTES || body.len() < 9 {
        return Err(Error::Malformed);
    }
    if u64::from_le_bytes(body[..8].try_into().unwrap()) != team {
        return Err(Error::Malformed);
    }
    let mut offset = 8;
    let count = varint(body, &mut offset)?;
    if count > 100000 {
        return Err(Error::Limit);
    }
    let mut found = false;
    for _ in 0..count {
        let (fields, used) =
            nim_property::decode(&body[offset..], 128, MAX_BYTES).map_err(|_| Error::Malformed)?;
        offset += used;
        let accounts = fields.iter().filter(|(id, _)| *id == 3).collect::<Vec<_>>();
        if accounts.len() != 1 {
            return Err(Error::Malformed);
        }
        found |= accounts[0].1.as_slice() == account.as_bytes();
    }
    if offset != body.len() {
        return Err(Error::Malformed);
    }
    Ok(found)
}

pub fn delivery_receipt(packet: &Packet, serial: u16, client_id: &str) -> Result<String, Error> {
    if !matches!(
        (packet.header.service, packet.header.command),
        (7, 1) | (8, 2)
    ) || packet.header.serial != serial
    {
        return Err(Error::Malformed);
    }
    if packet.header.response_code != 200 {
        return Err(Error::Rejected(packet.header.response_code));
    }
    let (fields, used) =
        nim_property::decode(&packet.body, 128, MAX_BYTES).map_err(|_| Error::Malformed)?;
    if used != packet.body.len() {
        return Err(Error::Malformed);
    }
    let mut ids = BTreeMap::new();
    for (id, value) in fields {
        if ids.insert(id, value).is_some() {
            return Err(Error::Malformed);
        }
    }
    // SDK sendTeamMsg confirmation contains only time (7) and server ID (12).
    // Correlate by exact route/serial in Client; validate the client ID if echoed.
    if ids
        .get(&11)
        .is_some_and(|id| id.as_slice() != client_id.as_bytes())
    {
        return Err(Error::Malformed);
    }
    if ids
        .get(&7)
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_none()
    {
        return Err(Error::Malformed);
    }
    let id = String::from_utf8(ids.remove(&12).ok_or(Error::Malformed)?)
        .map_err(|_| Error::Malformed)?;
    if id.parse::<u64>().ok().filter(|v| *v > 0).is_none() {
        return Err(Error::Malformed);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mute_wire_and_readback_validate_team_and_account() {
        assert_eq!(
            member_mute_request(12, 7, "a", true).unwrap(),
            vec![0, 8, 25, 12, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 1, 97, 1, 0, 0, 0]
        );
        let mut body = 7u64.to_le_bytes().to_vec();
        body.push(1);
        body.extend(nim_property::encode(&[(3, b"a".to_vec())], 128, MAX_BYTES).unwrap());
        assert_eq!(muted_member_present(&body, 7, "a"), Ok(true));
        assert_eq!(muted_member_present(&body, 7, "b"), Ok(false));
        assert!(muted_member_present(&body, 8, "a").is_err());
        for end in 0..body.len() {
            assert!(muted_member_present(&body[..end], 7, "a").is_err());
        }
    }
    #[test]
    fn group_recall_wire_and_notification_are_scoped() {
        let bytes = recall_request(12, "team", "sender", "client", "123", 1000).unwrap();
        let frame = crate::nim_transport::decode_message(&bytes, MAX_BYTES).unwrap();
        assert_eq!(
            (
                frame.header.service,
                frame.header.command,
                frame.header.serial
            ),
            (7, 13, 12)
        );
        let (fields, used) = nim_property::decode(&frame.body, 128, MAX_BYTES).unwrap();
        assert_eq!(used, frame.body.len());
        assert_eq!(
            fields,
            vec![
                (0, b"1000".to_vec()),
                (1, b"8".to_vec()),
                (2, b"team".to_vec()),
                (3, b"sender".to_vec()),
                (10, b"client".to_vec()),
                (11, b"123".to_vec()),
                (16, b"sender".to_vec())
            ]
        );
        let event = decode(&packet(7, 14, frame.body.clone())).unwrap();
        assert_eq!(
            event,
            Event::Recalled {
                target: "team".into(),
                server_id: "123".into()
            }
        );
        for size in 0..frame.body.len() {
            assert!(decode(&packet(7, 14, frame.body[..size].to_vec())).is_err());
        }
        assert!(recall_request(12, "team", "sender", "client", "0", 1000).is_err());
        assert!(recall_request(12, "team", "sender", "", "123", 1000).is_err());
    }
    fn packet(service: u8, command: u8, body: Vec<u8>) -> Packet {
        let mut frame = Header::request(service, command, 9).to_vec();
        frame.extend(body);
        crate::nim_transport::decode_message(&frame, MAX_BYTES).unwrap()
    }
    fn fields() -> Vec<u8> {
        nim_property::encode(
            &[
                (0, b"1".to_vec()),
                (1, b"group".to_vec()),
                (2, b"sender".to_vec()),
                (7, b"1000".to_vec()),
                (8, b"100".to_vec()),
                (10, b"opaque".to_vec()),
                (11, b"client".to_vec()),
                (12, (u64::MAX - 1).to_string().into_bytes()),
            ],
            128,
            MAX_BYTES,
        )
        .unwrap()
    }
    #[test]
    fn sdk_group_send_and_minimal_confirmation() {
        let bytes = custom_request(9, Scene::Group, "group", "client", "opaque").unwrap();
        let request = crate::nim_transport::decode_message(&bytes, MAX_BYTES).unwrap();
        assert_eq!((request.header.service, request.header.command), (8, 2));
        let body = nim_property::encode(
            &[(7, b"1789573000000".to_vec()), (12, b"123456789".to_vec())],
            128,
            MAX_BYTES,
        )
        .unwrap();
        assert_eq!(
            delivery_receipt(&packet(8, 2, body.clone()), 9, "client").unwrap(),
            "123456789"
        );
        assert!(delivery_receipt(&packet(8, 2, body), 10, "client").is_err());
    }
    #[test]
    fn sdk_realtime_notification_unwraps_bounded_inner_header() {
        let mut body = 123456789u64.to_le_bytes().to_vec();
        body.extend(Header::request(8, 3, 0));
        body.extend(fields());
        assert!(matches!(decode(&packet(4,1,body.clone())),Ok(Event::Messages(m)) if m.len()==1));
        for n in 0..14 {
            assert!(decode(&packet(4, 1, body[..n].to_vec())).is_err());
        }
        let mut nested = 1u64.to_le_bytes().to_vec();
        nested.extend(Header::request(4, 1, 0));
        nested.extend(body);
        assert!(decode(&packet(4, 1, nested)).is_err());
    }
    #[test]
    fn messages_preserve_ids_and_opaque_custom_content() {
        let Event::Messages(messages) = decode(&packet(8, 3, fields())).unwrap() else {
            panic!()
        };
        assert_eq!(messages[0].server_id, (u64::MAX - 1).to_string());
        assert_eq!(messages[0].body, "");
        assert_eq!(messages[0].attachment, "opaque");
        assert!(!format!("{:?}", messages[0]).contains("opaque"));
        let mut batch = vec![2];
        batch.extend(fields());
        batch.extend(fields());
        let Event::Messages(messages) = decode(&packet(4, 4, batch)).unwrap() else {
            panic!()
        };
        assert_eq!(messages.len(), 2);
    }
    #[test]
    fn rejects_truncated_and_trailing_data() {
        let valid = fields();
        for end in 0..valid.len() {
            assert!(decode(&packet(8, 3, valid[..end].to_vec())).is_err());
        }
        let mut trailing = valid;
        trailing.push(0);
        assert!(decode(&packet(8, 3, trailing)).is_err());
        assert_eq!(
            decode(&packet(4, 4, vec![0xff, 0xff, 0xff, 0xff, 0x7f])),
            Err(Error::Malformed)
        );
    }
    #[test]
    fn ack_routes_and_64_bit_ids_are_exact() {
        let ack = acknowledge(9, Scene::Group, &[(u64::MAX - 1).to_string()]).unwrap();
        assert_eq!(
            &ack[6..],
            &[8, 3, 1, 254, 255, 255, 255, 255, 255, 255, 255]
        );
        assert_eq!(
            &acknowledge(9, Scene::Private, &["1".into()]).unwrap()[6..9],
            &[7, 2, 1]
        );
        assert!(acknowledge(9, Scene::Private, &["not-an-id".into()]).is_err());
    }
    #[test]
    fn receipt_must_match_command_serial_and_client_id() {
        assert_eq!(
            delivery_receipt(&packet(7, 1, fields()), 9, "client").unwrap(),
            (u64::MAX - 1).to_string()
        );
        assert!(delivery_receipt(&packet(7, 1, fields()), 10, "client").is_err());
        assert!(delivery_receipt(&packet(7, 1, fields()), 9, "different").is_err());
        assert!(delivery_receipt(&packet(8, 3, fields()), 9, "client").is_err());
    }
}
