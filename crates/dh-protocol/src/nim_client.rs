//! Explicit-endpoint transport. Credential acquisition and discovery remain external.
use crate::{
    credentials::{CredentialCoordinator, Snapshot},
    nim_login,
    nim_transport::{self, Packet},
};
use futures_util::{SinkExt, StreamExt};
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    Connector, MaybeTlsStream, WebSocketStream,
};

const MAX_MESSAGE: usize = 1024 * 1024;
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Reviewed SDK LBS contract; no token is sent to discovery.
pub async fn discover(
    account: &str,
    app_key: &str,
    deadline: Duration,
) -> Result<Vec<String>, Error> {
    if account.is_empty() || app_key.is_empty() {
        return Err(Error::InvalidLogin);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(deadline)
        .build()
        .map_err(|_| Error::Transport)?;
    let mut response = client
        .get("https://lbs.netease.im/lbs/webconf.jsp")
        .query(&[
            ("id", account),
            ("k", app_key),
            ("sv", "92114"),
            ("pv", "1"),
            ("networkType", "0"),
            ("hostEnv", "BROWSER"),
        ])
        .send()
        .await
        .map_err(|_| Error::Transport)?;
    if !response.status().is_success() {
        return Err(Error::InvalidResponse);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Transport)? {
        if bytes.len() + chunk.len() > 65536 {
            return Err(Error::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk)
    }
    let data: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::InvalidResponse)?;
    let mut endpoints = Vec::new();
    for key in ["link", "link.default"] {
        if let Some(links) = data
            .get("common")
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_array())
        {
            for link in links.iter().rev().take(16) {
                let host = link.as_str().ok_or(Error::InvalidResponse)?;
                let mut url: reqwest::Url = if host.contains("://") {
                    host.to_owned()
                } else {
                    format!("https://{host}")
                }
                .parse()
                .map_err(|_| Error::InvalidResponse)?;
                if url.scheme() != "https"
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.host_str().is_none()
                    || url.path() != "/"
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(Error::InvalidResponse);
                }
                url.set_scheme("wss").map_err(|_| Error::InvalidResponse)?;
                url.set_path("/websocket");
                if !endpoints.contains(&url.to_string()) {
                    endpoints.push(url.to_string());
                }
            }
        }
    }
    if endpoints.is_empty() {
        return Err(Error::InvalidResponse);
    }
    Ok(endpoints)
}

/// Errors intentionally exclude URLs, payloads and upstream error strings.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidEndpoint,
    InvalidLogin,
    StaleCredentials,
    Timeout,
    Transport,
    Closed,
    InvalidResponse,
    UnexpectedPacket { service: u8, command: u8 },
    InvalidAuthBody,
    Rejected(u16),
    Kicked,
    QueueFull,
}

/// Owns one authenticated socket. A credential change invalidates this connection.
/// Push payloads remain opaque until decoded by the application adapter.
pub struct Client {
    socket: Socket,
    credentials: Arc<CredentialCoordinator>,
    snapshot: Snapshot,
    deadline: Duration,
    pending: VecDeque<Packet>,
    pending_bytes: usize,
    next_serial: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent { server_id: String },
    Rejected { code: u16 },
    Unknown,
}

impl Client {
    /// Fields must come from verified account/SDK configuration. The coordinator
    /// supplies the token so a caller cannot accidentally use an older token.
    pub async fn connect(
        endpoint: &str,
        fields: &[(&str, &str)],
        credentials: Arc<CredentialCoordinator>,
        deadline: Duration,
    ) -> Result<Self, Error> {
        if !endpoint.starts_with("wss://") {
            return Err(Error::InvalidEndpoint);
        }
        Self::connect_inner(endpoint, fields, credentials, deadline).await
    }

    async fn connect_inner(
        endpoint: &str,
        fields: &[(&str, &str)],
        credentials: Arc<CredentialCoordinator>,
        deadline: Duration,
    ) -> Result<Self, Error> {
        let snapshot = credentials
            .snapshot()
            .await
            .ok_or(Error::StaleCredentials)?;
        if fields.iter().any(|(name, _)| *name == "token") {
            return Err(Error::InvalidLogin);
        }
        let mut login_fields = fields.to_vec();
        login_fields.push(("token", snapshot.nim.expose()));
        let login = nim_login::encode(1, &login_fields).map_err(|_| Error::InvalidLogin)?;
        let socket = timeout(deadline, async {
            let config = WebSocketConfig::default()
                .max_message_size(Some(MAX_MESSAGE))
                .max_frame_size(Some(MAX_MESSAGE));
            let roots =
                rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|_| Error::Transport)?
            .with_root_certificates(roots)
            .with_no_client_auth();
            let (mut socket, _) = connect_async_tls_with_config(
                endpoint,
                Some(config),
                false,
                Some(Connector::Rustls(Arc::new(tls))),
            )
            .await
            .map_err(|_| Error::Transport)?;
            if !credentials.is_current(&snapshot).await {
                return Err(Error::StaleCredentials);
            }
            socket
                .send(Message::Binary(login.to_vec().into()))
                .await
                .map_err(|_| Error::Transport)?;
            let packet = read_packet(&mut socket).await?;
            if packet.header.service != 2 || packet.header.command != 3 || packet.header.serial != 1
            {
                return Err(Error::UnexpectedPacket {
                    service: packet.header.service,
                    command: packet.header.command,
                });
            }
            if packet.header.response_code != 200 {
                return Err(Error::Rejected(packet.header.response_code));
            }
            nim_login::decode_auth(&packet.body).map_err(|_| Error::InvalidAuthBody)?;
            if !credentials.is_current(&snapshot).await {
                return Err(Error::StaleCredentials);
            }
            Ok(socket)
        })
        .await
        .map_err(|_| Error::Timeout)??;
        Ok(Self {
            socket,
            credentials,
            snapshot,
            deadline,
            pending: VecDeque::new(),
            pending_bytes: 0,
            next_serial: 10,
        })
    }

    /// Returns one bounded binary packet; callers must decode and acknowledge
    /// provider push messages before claiming application-level delivery.
    pub async fn receive(&mut self) -> Result<Packet, Error> {
        if !self.credentials.is_current(&self.snapshot).await {
            return Err(Error::StaleCredentials);
        }
        if let Some(packet) = self.pending.pop_front() {
            self.pending_bytes -= packet.body.len();
            return Ok(packet);
        }
        let packet = timeout(self.deadline, read_packet(&mut self.socket))
            .await
            .map_err(|_| Error::Timeout)??;
        if !self.credentials.is_current(&self.snapshot).await {
            return Err(Error::StaleCredentials);
        }
        Ok(packet)
    }

    pub async fn close(mut self) -> Result<(), Error> {
        timeout(self.deadline, self.socket.close(None))
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Transport)
    }

    fn serial(&mut self) -> u16 {
        let serial = self.next_serial;
        self.next_serial = if serial == 32767 { 10 } else { serial + 1 };
        serial
    }

    fn retain(&mut self, packet: Packet) -> Result<(), Error> {
        if self.pending.len() >= 1024 || self.pending_bytes + packet.body.len() > MAX_MESSAGE {
            return Err(Error::QueueFull);
        }
        self.pending_bytes += packet.body.len();
        self.pending.push_back(packet);
        Ok(())
    }

    /// A write may have reached the server even when the transport reports failure.
    /// Callers must persist their client_id before this call and never retry Unknown.
    pub async fn send_custom(
        &mut self,
        scene: crate::nim_message::Scene,
        to: &str,
        client_id: &str,
        content: &str,
    ) -> Result<Delivery, Error> {
        if !self.credentials.is_current(&self.snapshot).await {
            return Err(Error::StaleCredentials);
        }
        let serial = self.serial();
        let frame = crate::nim_message::custom_request(serial, scene, to, client_id, content)
            .map_err(|_| Error::InvalidResponse)?;
        let result = timeout(self.deadline, async {
            self.socket
                .send(Message::Binary(frame.into()))
                .await
                .map_err(|_| Error::Transport)?;
            loop {
                let packet = read_packet(&mut self.socket).await?;
                if !self.credentials.is_current(&self.snapshot).await {
                    return Err(Error::StaleCredentials);
                }
                let (service, command) = crate::nim_message::send_route(scene);
                if (
                    packet.header.service,
                    packet.header.command,
                    packet.header.serial,
                ) == (service, command, serial)
                {
                    return match crate::nim_message::delivery_receipt(&packet, serial, client_id) {
                        Ok(server_id) => Ok(Delivery::Sent { server_id }),
                        Err(crate::nim_message::Error::Rejected(code)) => {
                            Ok(Delivery::Rejected { code })
                        }
                        Err(_) => Ok(Delivery::Unknown),
                    };
                }
                self.retain(packet)?;
            }
        })
        .await;
        Ok(match result {
            Ok(Ok(delivery)) => delivery,
            _ => Delivery::Unknown,
        })
    }

    /// A successful response confirms acceptance, not disappearance on peer devices.
    pub async fn recall_group(
        &mut self,
        target: &str,
        from: &str,
        client: &str,
        server: &str,
        time: u64,
    ) -> Result<Delivery, Error> {
        if !self.credentials.is_current(&self.snapshot).await {
            return Err(Error::StaleCredentials);
        }
        let serial = self.serial();
        let frame = crate::nim_message::recall_request(serial, target, from, client, server, time)
            .map_err(|_| Error::InvalidResponse)?;
        let result = timeout(self.deadline, async {
            self.socket
                .send(Message::Binary(frame.into()))
                .await
                .map_err(|_| Error::Transport)?;
            loop {
                let packet = read_packet(&mut self.socket).await?;
                if !self.credentials.is_current(&self.snapshot).await {
                    return Err(Error::StaleCredentials);
                }
                if (
                    packet.header.service,
                    packet.header.command,
                    packet.header.serial,
                ) == (7, 13, serial)
                {
                    return Ok(if packet.header.response_code == 200 {
                        Delivery::Sent {
                            server_id: server.into(),
                        }
                    } else {
                        Delivery::Rejected {
                            code: packet.header.response_code,
                        }
                    });
                }
                self.retain(packet)?;
            }
        })
        .await;
        Ok(match result {
            Ok(Ok(delivery)) => delivery,
            _ => Delivery::Unknown,
        })
    }

    pub async fn set_member_mute_verified(
        &mut self,
        team: u64,
        account: &str,
        muted: bool,
    ) -> Result<bool, Error> {
        let serial = self.serial();
        let frame = crate::nim_message::member_mute_request(serial, team, account, muted)
            .map_err(|_| Error::InvalidResponse)?;
        self.exchange_management(frame, 8, 25, serial).await?;
        let serial = self.serial();
        let mut frame = crate::nim_header::Header::request(8, 27, serial).to_vec();
        frame.extend(team.to_le_bytes());
        let packet = self.exchange_management(frame, 8, 27, serial).await?;
        let actual = crate::nim_message::muted_member_present(&packet.body, team, account)
            .map_err(|_| Error::InvalidResponse)?;
        Ok(actual == muted)
    }

    async fn exchange_management(
        &mut self,
        frame: Vec<u8>,
        service: u8,
        command: u8,
        serial: u16,
    ) -> Result<Packet, Error> {
        if !self.credentials.is_current(&self.snapshot).await {
            return Err(Error::StaleCredentials);
        }
        timeout(self.deadline, async {
            self.socket
                .send(Message::Binary(frame.into()))
                .await
                .map_err(|_| Error::Transport)?;
            loop {
                let packet = read_packet(&mut self.socket).await?;
                if !self.credentials.is_current(&self.snapshot).await {
                    return Err(Error::StaleCredentials);
                }
                if (
                    packet.header.service,
                    packet.header.command,
                    packet.header.serial,
                ) == (service, command, serial)
                {
                    if packet.header.response_code != 200 {
                        return Err(Error::Rejected(packet.header.response_code));
                    }
                    return Ok(packet);
                }
                self.retain(packet)?;
            }
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// No protocol response is defined for batchMarkRead. Success only confirms
    /// local socket flush, not remote receipt. Repeating this ACK is idempotent.
    pub async fn acknowledge(
        &mut self,
        scene: crate::nim_message::Scene,
        ids: &[String],
    ) -> Result<(), Error> {
        if !self.credentials.is_current(&self.snapshot).await {
            return Err(Error::StaleCredentials);
        }
        let frame = crate::nim_message::acknowledge(self.serial(), scene, ids)
            .map_err(|_| Error::InvalidResponse)?;
        timeout(
            self.deadline,
            self.socket.send(Message::Binary(frame.into())),
        )
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Transport)
    }

    pub async fn poll(&mut self, wait: Duration) -> Result<Option<Packet>, Error> {
        match timeout(wait, self.receive()).await {
            Ok(Ok(packet)) => Ok(Some(packet)),
            Ok(Err(Error::Timeout)) | Err(_) => Ok(None),
            Ok(Err(error)) => Err(error),
        }
    }

    pub async fn start_sync(&mut self, cursor: u64) -> Result<(), Error> {
        if !self.credentials.is_current(&self.snapshot).await {
            return Err(Error::StaleCredentials);
        }
        let frame = crate::nim_message::sync_request(self.serial(), cursor);
        timeout(
            self.deadline,
            self.socket.send(Message::Binary(frame.into())),
        )
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Transport)
    }

    /// Application heartbeat (link/1_2), distinct from a WebSocket Ping.
    /// Interleaved pushes are retained in a bounded queue for receive().
    pub async fn heartbeat(&mut self, serial: u16) -> Result<(), Error> {
        if !self.credentials.is_current(&self.snapshot).await {
            return Err(Error::StaleCredentials);
        }
        let frame = crate::nim_header::Header::request(1, 2, serial);
        timeout(
            self.deadline,
            self.socket.send(Message::Binary(frame.to_vec().into())),
        )
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Transport)?;
        timeout(self.deadline, async {
            loop {
                let packet = read_packet(&mut self.socket).await?;
                if !self.credentials.is_current(&self.snapshot).await {
                    return Err(Error::StaleCredentials);
                }
                if packet.header.service == 1 && packet.header.command == 2 {
                    if packet.header.serial != serial || !packet.body.is_empty() {
                        return Err(Error::InvalidResponse);
                    }
                    if packet.header.response_code != 200 {
                        return Err(Error::Rejected(packet.header.response_code));
                    }
                    return Ok(());
                }
                self.retain(packet)?;
            }
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}

async fn read_packet(socket: &mut Socket) -> Result<Packet, Error> {
    loop {
        match socket
            .next()
            .await
            .ok_or(Error::Closed)?
            .map_err(|_| Error::Transport)?
        {
            Message::Binary(bytes) => {
                let packet = nim_transport::decode_message(&bytes, MAX_MESSAGE)
                    .map_err(|_| Error::InvalidResponse)?;
                if packet.header.service == 2 && packet.header.command == 5 {
                    return Err(Error::Kicked);
                }
                return Ok(packet);
            }
            Message::Ping(_) => socket.flush().await.map_err(|_| Error::Transport)?,
            Message::Pong(_) => {}
            Message::Close(_) => return Err(Error::Closed),
            _ => return Err(Error::InvalidResponse),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{credentials::Secret, nim_header::Header, nim_property};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn heartbeat_preserves_interleaved_push_and_rejects_kick() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let credentials = Arc::new(CredentialCoordinator::default());
        credentials
            .login(Secret::new("b".into()), Secret::new("n".into()))
            .await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.next().await.unwrap().unwrap();
            let response = |service, command, serial, body: &[u8]| {
                let mut p = Header::request(service, command, serial).to_vec();
                p[5] = 2;
                p.extend_from_slice(&200u16.to_le_bytes());
                p.extend_from_slice(body);
                p
            };
            let auth = nim_property::encode(&[(102, b"connection".to_vec())], 128, 1024).unwrap();
            ws.send(Message::Binary(response(2, 3, 1, &auth).into()))
                .await
                .unwrap();
            ws.next().await.unwrap().unwrap();
            ws.send(Message::Binary(response(4, 1, 99, b"opaque-push").into()))
                .await
                .unwrap();
            ws.send(Message::Binary(response(1, 2, 2, &[]).into()))
                .await
                .unwrap();
            ws.next().await.unwrap().unwrap();
            ws.send(Message::Binary(response(2, 5, 100, &[]).into()))
                .await
                .unwrap();
        });
        let mut client = Client::connect_inner(
            &endpoint,
            &[
                ("account", "a"),
                ("appKey", "k"),
                ("deviceId", "d"),
                ("session", "s"),
            ],
            credentials,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        client.heartbeat(2).await.unwrap();
        let push = client.receive().await.unwrap();
        assert_eq!(push.body.as_slice(), b"opaque-push");
        assert_eq!(client.heartbeat(3).await, Err(Error::Kicked));
        server.await.unwrap();
    }

    async fn exercise(code: u16, serial: u16, logout: bool) -> Result<Client, Error> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let credentials = Arc::new(CredentialCoordinator::default());
        credentials
            .login(
                Secret::new("business".into()),
                Secret::new("synthetic-token".into()),
            )
            .await;
        let c = credentials.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let request = ws.next().await.unwrap().unwrap().into_data();
            let p = nim_transport::decode_message(&request, MAX_MESSAGE).unwrap();
            let (fields, _) = nim_property::decode(&p.body, 25, MAX_MESSAGE).unwrap();
            assert_eq!(
                fields.iter().find(|(id, _)| *id == 1000).unwrap().1,
                b"synthetic-token"
            );
            if logout {
                c.logout().await;
            }
            let mut reply = Header::request(2, 3, serial).to_vec();
            reply[5] = 2;
            reply.extend_from_slice(&code.to_le_bytes());
            reply.extend(
                nim_property::encode(&[(102, b"connection".to_vec())], 25, MAX_MESSAGE).unwrap(),
            );
            reply.extend_from_slice(&[0, 0]);
            ws.send(Message::Binary(reply.into())).await.unwrap();
        });
        let result = Client::connect_inner(
            &endpoint,
            &[
                ("account", "synthetic"),
                ("appKey", "app"),
                ("deviceId", "device"),
                ("session", "session"),
            ],
            credentials,
            Duration::from_secs(2),
        )
        .await;
        server.await.unwrap();
        result
    }

    #[tokio::test]
    async fn authenticates_only_matching_success_with_current_credentials() {
        assert!(exercise(200, 1, false).await.is_ok());
        assert!(matches!(
            exercise(401, 1, false).await,
            Err(Error::Rejected(401))
        ));
        assert!(matches!(
            exercise(200, 2, false).await,
            Err(Error::UnexpectedPacket { .. })
        ));
        assert!(matches!(
            exercise(200, 1, true).await,
            Err(Error::StaleCredentials)
        ));
    }
    #[tokio::test]
    async fn bounds_silent_peer_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let credentials = Arc::new(CredentialCoordinator::default());
        credentials
            .login(Secret::new("b".into()), Secret::new("n".into()))
            .await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert!(ws.next().await.is_some());
            let _ = ws.next().await;
        });
        let result = Client::connect_inner(
            &endpoint,
            &[
                ("account", "a"),
                ("appKey", "k"),
                ("deviceId", "d"),
                ("session", "s"),
            ],
            credentials,
            Duration::from_millis(100),
        )
        .await;
        assert!(matches!(result, Err(Error::Timeout)));
        timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn refuses_plaintext_production_endpoint() {
        assert!(matches!(
            Client::connect(
                "ws://127.0.0.1",
                &[],
                Arc::new(CredentialCoordinator::default()),
                Duration::from_secs(1)
            )
            .await,
            Err(Error::InvalidEndpoint)
        ));
    }
}
