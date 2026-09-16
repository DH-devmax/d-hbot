//! Explicit configuration business client. Never falls back to the official app.
use crate::{
    business_wire::{self, Binding},
    request_metadata,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use crypto_box::{aead::rand_core::OsRng, PublicKey, SecretKey};
use ed25519_dalek::{Signer, SigningKey};
use prost::Message;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Url,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Configuration,
    Clock,
    Encoding,
    Transport,
    Http(u16),
    Response,
    ResponseStage(&'static str),
    Business(i64),
    Expired,
    PasswordAttemptsExceeded,
    InvalidCredentials,
    InvalidSmsCode,
    SmsExpired,
}

fn login_feedback(message: Option<&str>) -> Option<Error> {
    match message {
        Some("密码尝试次数过多，请通过短信验证码登录") => {
            Some(Error::PasswordAttemptsExceeded)
        }
        Some("账号或密码错误") | Some("密码错误") | Some("账号或密码错误, 请重新输入") => {
            Some(Error::InvalidCredentials)
        }
        Some("验证码错误") | Some("短信验证码错误") => Some(Error::InvalidSmsCode),
        Some("验证码已过期") | Some("短信验证码已过期") => Some(Error::SmsExpired),
        _ => None,
    }
}
#[cfg(test)]
#[test]
fn login_feedback_never_returns_unrecognized_provider_text() {
    assert_eq!(
        login_feedback(Some("密码尝试次数过多，请通过短信验证码登录")),
        Some(Error::PasswordAttemptsExceeded)
    );
    assert_eq!(
        login_feedback(Some("密码错误")),
        Some(Error::InvalidCredentials)
    );
    assert_eq!(
        login_feedback(Some("验证码错误")),
        Some(Error::InvalidSmsCode)
    );
    assert_eq!(
        login_feedback(Some("验证码已过期")),
        Some(Error::SmsExpired)
    );
    assert_eq!(login_feedback(Some("private unknown response")), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new(Config {
            origin: "https://example.invalid".parse().unwrap(),
            headers: BTreeMap::new(),
            metadata: [0; 14],
            signing_seed: Zeroizing::new([7; 32]),
            body_key: Zeroizing::new([8; 32]),
            account_mac: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn sealed_login_is_bound_to_attempt_and_identity_before_install() {
        let mut client = client();
        assert_eq!(client.groups().await, Err(Error::Expired));
        let signer = SigningKey::from_bytes(&client.config.signing_seed);
        let secret = SecretKey::from(signer.to_scalar_bytes());
        let exchange = LoginExchange {
            device_tag: 42,
            kind: 1,
            public_key: signer.verifying_key().to_bytes().to_vec(),
            seconds: 100,
            correlation: 200,
            version: 1,
            flag: false,
        };
        for changed in 0..5 {
            let token = LoginToken {
                seconds: if changed == 0 { 101 } else { 100 },
                correlation: if changed == 1 { 201 } else { 200 },
                config: vec![],
                mac: vec![3; if changed == 2 { 15 } else { 16 }],
                kind: 1,
                uid: if changed == 3 { 124 } else { 123 },
            };
            let sealed = secret
                .public_key()
                .seal(&mut OsRng, &token.encode_to_vec())
                .unwrap();
            let data = serde_json::json!({"uid":123,"jwtToken":if changed==4{""}else{"synthetic-jwt"},"token":URL_SAFE_NO_PAD.encode(sealed)});
            assert_eq!(
                client.install_login(&data, &signer, &exchange),
                Err(Error::ResponseStage(
                    [
                        "login_seconds",
                        "login_correlation",
                        "login_mac_length",
                        "login_identity",
                        "login_jwt"
                    ][changed]
                ))
            );
            assert!(client.invalid);
            assert!(client.config.account_mac.is_none());
            assert!(client.config.headers.is_empty());
        }
        let token = LoginToken {
            seconds: 100,
            correlation: 200,
            config: vec![],
            mac: vec![3; 32],
            kind: 1,
            uid: 123,
        };
        let sealed = secret
            .public_key()
            .seal(&mut OsRng, &token.encode_to_vec())
            .unwrap();
        let data = serde_json::json!({"uid":123,"jwtToken":"synthetic-jwt","token":URL_SAFE_NO_PAD.encode(sealed)});
        client.install_login(&data, &signer, &exchange).unwrap();
        assert!(!client.invalid);
        assert_eq!(client.config.metadata[3], 123);
        assert_eq!(client.config.account_mac.as_deref(), Some(&[3; 16]));
        assert_eq!(client.config.headers.get("x-jwt").unwrap(), "synthetic-jwt");
        let snapshot = client.export_session().unwrap();
        let mut restored = Client::new(Config {
            origin: "https://example.invalid".parse().unwrap(),
            headers: BTreeMap::new(),
            metadata: [0; 14],
            signing_seed: Zeroizing::new([7; 32]),
            body_key: Zeroizing::new([8; 32]),
            account_mac: None,
        })
        .unwrap();
        restored.restore_session(&snapshot).unwrap();
        assert_eq!(restored.user_id().unwrap(), 123);
        assert_eq!(
            restored.config.headers.get("x-jwt"),
            client.config.headers.get("x-jwt")
        );
        assert_eq!(restored.config.account_mac, client.config.account_mac);
        restored.config.origin = "https://another.invalid".parse().unwrap();
        assert_eq!(
            restored.restore_session(&snapshot),
            Err(Error::Configuration)
        );
    }
}

/// Secrets have no Debug/Serialize implementation. Persist only through an
/// explicitly chosen encrypted credential store.
pub struct Config {
    pub origin: Url,
    pub headers: BTreeMap<String, String>,
    pub metadata: [u64; 14],
    pub signing_seed: Zeroizing<[u8; 32]>,
    pub body_key: Zeroizing<[u8; 32]>,
    pub account_mac: Option<Zeroizing<[u8; 16]>>,
}
pub struct Client {
    config: Config,
    http: reqwest::Client,
    last_request: u64,
    invalid: bool,
}

#[derive(Clone, PartialEq, Message)]
struct LoginExchange {
    #[prost(uint64, tag = "1")]
    device_tag: u64,
    #[prost(int32, tag = "2")]
    kind: i32,
    #[prost(bytes = "vec", tag = "3")]
    public_key: Vec<u8>,
    #[prost(int64, tag = "4")]
    seconds: i64,
    #[prost(int64, tag = "5")]
    correlation: i64,
    #[prost(int32, tag = "6")]
    version: i32,
    #[prost(bool, tag = "7")]
    flag: bool,
}
#[derive(Clone, PartialEq, Message)]
struct LoginToken {
    #[prost(int64, tag = "1")]
    seconds: i64,
    #[prost(int64, tag = "2")]
    correlation: i64,
    #[prost(bytes = "vec", tag = "3")]
    config: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    mac: Vec<u8>,
    #[prost(int32, tag = "5")]
    kind: i32,
    #[prost(int32, tag = "6")]
    uid: i32,
}
impl Client {
    /// Secret bytes for an encrypted local store only; never return through Web APIs.
    pub fn export_session(&self) -> Result<Zeroizing<Vec<u8>>, Error> {
        if self.invalid {
            return Err(Error::Expired);
        }
        let mac = self.config.account_mac.as_ref().ok_or(Error::Expired)?;
        serde_json::to_vec(&serde_json::json!({"origin":self.config.origin.as_str(),"uid":self.config.metadata[3],"kind":self.config.metadata[4],"jwt":self.config.headers.get("x-jwt"),"mac":URL_SAFE_NO_PAD.encode(**mac)})).map(Zeroizing::new).map_err(|_|Error::Encoding)
    }
    pub fn restore_session(&mut self, bytes: &[u8]) -> Result<(), Error> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Saved {
            origin: String,
            uid: u64,
            kind: u64,
            jwt: String,
            mac: String,
        }
        if bytes.len() > 16384 {
            return Err(Error::Configuration);
        }
        let saved: Saved = serde_json::from_slice(bytes).map_err(|_| Error::Configuration)?;
        if saved.origin != self.config.origin.as_str()
            || saved.uid == 0
            || saved.uid > i32::MAX as u64
            || saved.jwt.is_empty()
            || saved.jwt.len() > 8192
        {
            return Err(Error::Configuration);
        }
        HeaderValue::from_str(&saved.jwt).map_err(|_| Error::Configuration)?;
        let mac = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(&saved.mac)
                .map_err(|_| Error::Configuration)?,
        );
        let mac: [u8; 16] = mac
            .as_slice()
            .try_into()
            .map_err(|_| Error::Configuration)?;
        self.config.metadata[3] = saved.uid;
        self.config.metadata[4] = saved.kind;
        self.config.account_mac = Some(Zeroizing::new(mac));
        self.config
            .headers
            .insert("x-id".into(), saved.uid.to_string());
        self.config
            .headers
            .insert("x-token".into(), saved.uid.to_string());
        self.config.headers.insert("x-jwt".into(), saved.jwt);
        self.invalid = false;
        Ok(())
    }
    pub fn user_id(&self) -> Result<i64, Error> {
        if self.invalid {
            return Err(Error::Expired);
        }
        i64::try_from(self.config.metadata[3]).map_err(|_| Error::Configuration)
    }
    pub fn new(config: Config) -> Result<Self, Error> {
        if config.origin.scheme() != "https"
            || config.origin.host_str().is_none()
            || !config.origin.username().is_empty()
            || config.origin.password().is_some()
        {
            return Err(Error::Configuration);
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| Error::Configuration)?;
        let invalid = config.account_mac.is_none();
        Ok(Self {
            config,
            http,
            last_request: 0,
            invalid,
        })
    }
    pub async fn groups(&mut self) -> Result<Value, Error> {
        self.request(
            "/v1/group/get-group-list",
            &serde_json::json!({"v":"0"}),
            None,
        )
        .await
    }
    pub async fn members_page(
        &mut self,
        group_id: i64,
        cursor: Option<&str>,
    ) -> Result<Value, Error> {
        if group_id <= 0 || cursor.is_some_and(|s| s.len() > 8192) {
            return Err(Error::Configuration);
        }
        let mut params = serde_json::json!({"groupId":group_id,"v":"0"});
        if let Some(cursor) = cursor.filter(|s| !s.is_empty()) {
            params["cursor"] = Value::String(cursor.into());
        }
        self.request("/v1/group/get-group-members", &params, None)
            .await
    }
    /// Fixed reviewed business routes only; callers must enforce account/group permissions.
    pub async fn group_request(&mut self, path: &str, params: &Value) -> Result<Value, Error> {
        if !matches!(
            path,
            "/v1/group/notice-list"
                | "/v1/group/set-member-mute"
                | "/v1/group/member-mute-cancel"
                | "/v1/group/set-member-nickname"
                | "/v1/group/remove-group-member"
                | "/v1/group/set-group-mute"
                | "/v1/group/add-notice"
                | "/v1/group/notice-opt"
                | "/v1/group/notice-del"
        ) || params
            .get("groupId")
            .and_then(Value::as_i64)
            .is_none_or(|id| id <= 0)
        {
            return Err(Error::Configuration);
        }
        self.request(path, params, None).await
    }
    pub async fn refresh_nim(&mut self) -> Result<Zeroizing<String>, Error> {
        let data = self
            .request("/v1/user/RefreshToken", &serde_json::json!({}), None)
            .await?;
        let token = data
            .get("nimToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(Error::Response)?;
        Ok(Zeroizing::new(token.to_owned()))
    }
    pub async fn device_verification(
        &mut self,
        account: &str,
        password: &str,
        kind: &str,
    ) -> Result<Value, Error> {
        let mut params = serde_json::json!({"account":account,"passwd":password,"type":kind});
        let result = self
            .request("/v1/user/get-change-device-verify", &params, None)
            .await;
        use zeroize::Zeroize;
        if let Some(Value::String(value)) = params.get_mut("passwd") {
            value.zeroize();
        }
        result
    }
    pub async fn resend_device_sms(
        &mut self,
        phone: &Value,
        key: &str,
        validation: &str,
    ) -> Result<Value, Error> {
        let mut params = serde_json::json!({"ty":"VERIFY_FOR_LOGIN","phone":phone,"validateStr":validation,"Key":key});
        if key.is_empty() {
            params.as_object_mut().unwrap().remove("Key");
        }
        let result = self.request("/v1/verify/sms-anon", &params, None).await;
        use zeroize::Zeroize;
        for field in ["Key", "validateStr"] {
            if let Some(Value::String(value)) = params.get_mut(field) {
                value.zeroize();
            }
        }
        result
    }
    /// The validation result is supplied by the human challenge flow, never forged.
    /// Clears the old session first; failed attempts leave it unauthenticated.
    pub async fn login(&mut self, params: &Value, server_key: [u8; 32]) -> Result<Value, Error> {
        if !params.is_object() || params.get("type").and_then(Value::as_str).is_none() {
            return Err(Error::Configuration);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Clock)?;
        let correlation = i64::try_from(now.as_nanos()).map_err(|_| Error::Clock)?;
        let signer = SigningKey::from_bytes(&self.config.signing_seed);
        let tag = xxhash_rust::xxh3::xxh3_64(&signer.verifying_key().to_bytes());
        let exchange = LoginExchange {
            device_tag: tag,
            kind: 1,
            public_key: signer.verifying_key().to_bytes().to_vec(),
            seconds: now.as_secs() as i64,
            correlation,
            version: self.config.metadata[2] as i32,
            flag: false,
        };
        let wire = Zeroizing::new(exchange.encode_to_vec());
        let sealed = PublicKey::from(server_key)
            .seal(&mut OsRng, &wire)
            .map_err(|_| Error::Encoding)?;
        self.config.metadata[0] = tag;
        self.config.metadata[3] = 0;
        self.config.metadata[4] = 0;
        self.config.account_mac = None;
        for name in ["x-id", "x-token", "x-jwt"] {
            self.config.headers.remove(name);
        }
        self.invalid = true;
        let data = self
            .request("/v1/user/login", params, Some(&sealed))
            .await?;
        self.install_login(&data, &signer, &exchange)?;
        Ok(data)
    }
    fn install_login(
        &mut self,
        data: &Value,
        signer: &SigningKey,
        exchange: &LoginExchange,
    ) -> Result<(), Error> {
        let encoded = data
            .get("token")
            .and_then(Value::as_str)
            .ok_or(Error::ResponseStage("login_token_missing"))?;
        let secret = SecretKey::from(signer.to_scalar_bytes());
        let opened = crate::sealed_response::open_base64url(encoded.as_bytes(), &secret, 65536)
            .map_err(|_| Error::ResponseStage("login_token_open"))?;
        let token = LoginToken::decode(opened.as_slice())
            .map_err(|_| Error::ResponseStage("login_token_decode"))?;
        let uid = data
            .get("uid")
            .and_then(Value::as_i64)
            .ok_or(Error::ResponseStage("login_uid"))?;
        if token.seconds != exchange.seconds {
            return Err(Error::ResponseStage("login_seconds"));
        }
        if token.correlation != exchange.correlation {
            return Err(Error::ResponseStage("login_correlation"));
        }
        if i64::from(token.uid) != uid || uid <= 0 {
            return Err(Error::ResponseStage("login_identity"));
        }
        // Native +0x13094 stores the full field; +0x1ef00 passes its start to
        // SipHash-2-4, which consumes the first 16 bytes without an equality check.
        let mac = token
            .mac
            .get(..16)
            .ok_or(Error::ResponseStage("login_mac_length"))?;
        let jwt = data
            .get("jwtToken")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(Error::ResponseStage("login_jwt"))?;
        self.config.metadata[3] = uid as u64;
        self.config.metadata[4] = token.kind as u32 as u64;
        self.config.account_mac =
            Some(Zeroizing::new(mac.try_into().map_err(|_| Error::Response)?));
        self.config.headers.insert("x-id".into(), uid.to_string());
        self.config
            .headers
            .insert("x-token".into(), uid.to_string());
        self.config.headers.insert("x-jwt".into(), jwt.into());
        self.invalid = false;
        Ok(())
    }
    /// Limited to reviewed routes. No arbitrary write or automatic retry.
    async fn request(
        &mut self,
        path: &str,
        params: &Value,
        exchange: Option<&[u8]>,
    ) -> Result<Value, Error> {
        if self.invalid
            && !matches!(
                path,
                "/v1/user/login" | "/v1/user/get-change-device-verify" | "/v1/verify/sms-anon"
            )
        {
            return Err(Error::Expired);
        }
        if !matches!(
            path,
            "/v1/group/get-group-list"
                | "/v1/group/get-group-members"
                | "/v1/group/notice-list"
                | "/v1/group/set-member-mute"
                | "/v1/group/member-mute-cancel"
                | "/v1/group/set-member-nickname"
                | "/v1/group/remove-group-member"
                | "/v1/group/set-group-mute"
                | "/v1/group/add-notice"
                | "/v1/group/notice-opt"
                | "/v1/group/notice-del"
                | "/v1/user/RefreshToken"
                | "/v1/user/login"
                | "/v1/verify/sms-anon"
                | "/v1/user/get-change-device-verify"
        ) {
            return Err(Error::Configuration);
        }
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Clock)?;
        let id = u64::try_from(elapsed.as_nanos())
            .map_err(|_| Error::Clock)?
            .max(self.last_request.checked_add(1).ok_or(Error::Clock)?);
        self.last_request = id;
        let plaintext = Zeroizing::new(serde_json::to_vec(params).map_err(|_| Error::Encoding)?);
        let binding = Binding::new(id, &plaintext, self.config.metadata[0]);
        let body = business_wire::seal(*self.config.body_key, binding, &plaintext)
            .map_err(|_| Error::Encoding)?;
        let mut fields = self.config.metadata;
        fields[5] = id;
        fields[6] = plaintext.len() as u64;
        fields[7] = binding.plaintext_hash;
        fields[9] = elapsed.as_secs();
        fields[12] = 1;
        fields[8] = if let Some(key) = &self.config.account_mac {
            let mut input = [0; 24];
            for (part, v) in input.chunks_exact_mut(8).zip([fields[0], id, fields[9]]) {
                part.copy_from_slice(&v.to_le_bytes())
            }
            request_metadata::field9_mac(&input, key)
        } else {
            0
        };
        let metadata = request_metadata::encode(&fields).map_err(|_| Error::Encoding)?;
        let signer = SigningKey::from_bytes(&self.config.signing_seed);
        let mut headers = HeaderMap::new();
        for (name, value) in &self.config.headers {
            if matches!(
                name.as_str(),
                "x-version" | "x-device" | "x-id" | "x-token" | "x-jwt" | "content-type" | "accept"
            ) {
                let mut v = HeaderValue::from_str(value).map_err(|_| Error::Configuration)?;
                v.set_sensitive(true);
                headers.insert(
                    HeaderName::from_bytes(name.as_bytes()).map_err(|_| Error::Configuration)?,
                    v,
                );
            }
        }
        for (name, value) in [
            ("x-request", URL_SAFE_NO_PAD.encode(&*metadata)),
            (
                "x-seed",
                URL_SAFE_NO_PAD.encode(signer.verifying_key().to_bytes()),
            ),
            (
                "x-hash",
                URL_SAFE_NO_PAD.encode(signer.sign(&metadata).to_bytes()),
            ),
            ("x-trace-id", format!("{}.{}", fields[0], id)),
        ] {
            let mut v = HeaderValue::from_str(&value).map_err(|_| Error::Encoding)?;
            v.set_sensitive(true);
            headers.insert(HeaderName::from_static(name), v);
        }
        if let Some(exchange) = exchange {
            let mut v = HeaderValue::from_str(&URL_SAFE_NO_PAD.encode(exchange))
                .map_err(|_| Error::Encoding)?;
            v.set_sensitive(true);
            headers.insert("x-context", v);
        }
        let url = self
            .config
            .origin
            .join(path)
            .map_err(|_| Error::Configuration)?;
        let mut response = self
            .http
            .post(url)
            .headers(headers)
            .body(body.to_vec())
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        if response.status().as_u16() != 200 {
            return Err(Error::Http(response.status().as_u16()));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Transport)? {
            if bytes.len() + chunk.len() > business_wire::MAX_BODY + 4096 {
                return Err(Error::Response);
            }
            bytes.extend_from_slice(&chunk)
        }
        let plaintext = if bytes.starts_with(b"{\"") {
            bytes
        } else {
            business_wire::open(*self.config.body_key, binding, &bytes).map_err(|error| {
                Error::ResponseStage(match error {
                    business_wire::Error::Authentication => "body_authentication",
                    business_wire::Error::Compression => "body_compression",
                    business_wire::Error::TooLarge => "body_limit",
                    business_wire::Error::InvalidEnvelope => "body_envelope",
                })
            })?
        };
        let mut value: Value =
            serde_json::from_slice(&plaintext).map_err(|_| Error::ResponseStage("json"))?;
        let code = value
            .get("code")
            .and_then(Value::as_i64)
            .ok_or(Error::ResponseStage("business_code"))?;
        if code != 0 {
            if code == 401 {
                self.invalid = true;
            }
            if path == "/v1/user/login" && code != 1069 {
                if let Some(error) = login_feedback(value.get("msg").and_then(Value::as_str)) {
                    return Err(error);
                }
            }
            return Err(Error::Business(code));
        }
        Ok(value
            .get_mut("data")
            .ok_or(Error::ResponseStage("business_data"))?
            .take())
    }
}
