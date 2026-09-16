//! Synthetic-only vector for the independent native decoder oracle.
use base64::{engine::general_purpose::STANDARD, Engine};
use dh_protocol::message_envelope::{self, ApplicationMessage, Content, Source};
use prost::Message;
fn main() {
    let message = ApplicationMessage {
        from: Some(Source {
            id: 123,
            name: "Alice".into(),
        }),
        to: Some(Source {
            id: 456,
            name: "Test".into(),
        }),
        device: 1,
        session: 2,
        version: 2,
        created_at: 1700000000000,
        client_id: "synthetic-unique-id".into(),
        content: Some(Content {
            data: "DH BOT 测试 @DH hello".into(),
        }),
        ..Default::default()
    };
    let payload = message_envelope::seal([7; 32], &message, 8, 9, 1700000000).unwrap();
    println!(
        "{}",
        serde_json::json!({"payload":payload,"plaintext":STANDARD.encode(message.encode_to_vec())})
    );
}
