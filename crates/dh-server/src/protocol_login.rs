//! Rust service authentication. No browser/native execution or credential echo.
use super::{authorized, same_origin, Server};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use dh_protocol::business_client::{Client, Config};
use dh_protocol::{
    credentials::{CredentialCoordinator, Secret},
    nim_client::Client as NimClient,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    origin: String,
    headers: BTreeMap<String, String>,
    metadata: [u64; 14],
    signing_seed: String,
    body_key: String,
    #[serde(default)]
    message_key: String,
    server_key: String,
    pub captcha_id: String,
    app_key: String,
    device_id: String,
}
impl Drop for Deployment {
    fn drop(&mut self) {
        self.signing_seed.zeroize();
        self.body_key.zeroize();
        self.message_key.zeroize();
    }
}
fn key<const N: usize>(value: &str) -> Result<Zeroizing<[u8; N]>, ()> {
    let bytes = Zeroizing::new(STANDARD.decode(value).map_err(|_| ())?);
    Ok(Zeroizing::new(bytes.as_slice().try_into().map_err(|_| ())?))
}
impl Deployment {
    pub fn client(&self) -> Result<Client, ()> {
        let mut headers = self.headers.clone();
        headers.retain(|name, _| {
            matches!(
                name.as_str(),
                "x-version" | "x-device" | "accept" | "content-type"
            )
        });
        let mut metadata = self.metadata;
        metadata[3] = 0;
        metadata[4] = 0;
        metadata[8] = 0;
        Client::new(Config {
            origin: self.origin.parse().map_err(|_| ())?,
            headers,
            metadata,
            signing_seed: key(&self.signing_seed)?,
            body_key: key(&self.body_key)?,
            account_mac: None,
        })
        .map_err(|_| ())
    }
}
pub struct Session {
    saved_login: Option<SavedLogin>,
    vault: Option<crate::vault::Vault>,
    challenge: Option<SmsChallenge>,
    deployment: Deployment,
    pub(super) client: Option<Client>,
    attempts: Vec<Instant>,
    last_error: Option<String>,
    pub(super) nim: Option<NimClient>,
    pub(super) sync_started: bool,
    next_heartbeat: Instant,
    next_profile_sync: Instant,
    reconnect: ReconnectSchedule,
    pub(super) nim_account: Option<String>,
    pub(super) account_name: Option<String>,
    pub(super) epoch: Arc<std::sync::atomic::AtomicU64>,
    credentials: Arc<CredentialCoordinator>,
}
#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedLogin {
    account: String,
    password: String,
}
impl Drop for SavedLogin {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}
struct SmsChallenge {
    id: String,
    phone: Value,
    key: Zeroizing<String>,
    original_key: Zeroizing<String>,
    expires: Instant,
    resend_at: Instant,
}

#[derive(Default)]
struct ReconnectSchedule {
    failures: u32,
    next: Option<Instant>,
}
impl ReconnectSchedule {
    fn ready(&self, now: Instant) -> bool {
        self.next.is_none_or(|next| now >= next)
    }
    fn failed(&mut self, now: Instant) {
        let delay = (1u64 << self.failures.min(5)).min(30);
        self.failures = self.failures.saturating_add(1);
        self.next = Some(now + Duration::from_secs(delay));
    }
}
impl Drop for SmsChallenge {
    fn drop(&mut self) {
        if let Some(object) = self.phone.as_object_mut() {
            for value in object.values_mut() {
                if let Value::String(text) = value {
                    text.zeroize();
                }
            }
        }
    }
}
impl SmsChallenge {
    fn new(data: &Value) -> Result<Self, ()> {
        let phone = data
            .get("phone")
            .filter(|v| v.is_object())
            .ok_or(())?
            .clone();
        if phone
            .get("nationalNumber")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .is_none()
        {
            return Err(());
        }
        let key = data
            .pointer("/sms/key")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 8192)
            .ok_or(())?;
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            phone,
            key: Zeroizing::new(key.into()),
            original_key: Zeroizing::new(key.into()),
            expires: Instant::now() + Duration::from_secs(600),
            resend_at: Instant::now() + Duration::from_secs(60),
        })
    }
}
impl Session {
    pub(super) fn message_key(&self) -> Result<Zeroizing<[u8; 32]>, ()> { key(&self.deployment.message_key) }
    pub fn attach_vault(&mut self, vault: crate::vault::Vault) -> Result<(), &'static str> {
        if let Some(bytes) = vault.read("login.sealed")? {
            let saved: Option<SavedLogin> =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid saved login")?;
            if saved.as_ref().is_some_and(|v| {
                v.account.is_empty()
                    || v.account.len() > 64
                    || v.password.is_empty()
                    || v.password.len() > 1024
            }) {
                return Err("Invalid saved login");
            }
            self.saved_login = saved;
        }
        if let Some(bytes) = vault.read("session.sealed")? {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Saved {
                account: String,
                client: String,
                #[serde(default)]
                account_name: Option<String>,
            }
            if let Some(saved) = serde_json::from_slice::<Option<Saved>>(&bytes)
                .map_err(|_| "Invalid saved session")?
            {
                if saved.account.is_empty() || saved.account.len() > 128 {
                    return Err("Invalid saved account");
                }
                let mut client = self.deployment.client().map_err(|_| "Invalid deployment")?;
                let bytes = Zeroizing::new(
                    STANDARD
                        .decode(&saved.client)
                        .map_err(|_| "Invalid saved credentials")?,
                );
                client
                    .restore_session(&bytes)
                    .map_err(|_| "Invalid saved credentials")?;
                self.account_name = saved.account_name;
                self.nim_account = Some(saved.account);
                self.client = Some(client);
            }
        }
        self.vault = Some(vault);
        Ok(())
    }
    fn save_session(&self) -> Result<(), &'static str> {
        if let (Some(vault), Some(client), Some(account)) =
            (&self.vault, &self.client, &self.nim_account)
        {
            let bytes = client
                .export_session()
                .map_err(|_| "Cannot export session")?;
            let saved = Zeroizing::new(
                serde_json::to_vec(&json!({"account":account,"client":STANDARD.encode(&bytes),"account_name":self.account_name}))
                    .map_err(|_| "Cannot encode session")?,
            );
            vault.save("session.sealed", &saved)?;
        }
        Ok(())
    }
}
impl SmsChallenge {
    fn view(&self) -> Value {
        let number = self.phone["nationalNumber"].as_str().unwrap_or("");
        let suffix: String = number
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let masked = self
            .phone
            .get("maskedNationalNumber")
            .and_then(Value::as_str)
            .filter(|s| s.contains('*') && s.len() <= 64);
        let display = masked.map(str::to_owned).unwrap_or_else(|| {
            if suffix.len() == 4 {
                format!("****{suffix}")
            } else {
                "绑定手机号".into()
            }
        });
        json!({"id":self.id,"kind":"deviceSms","maskedPhone":display,"expiresIn":self.expires.saturating_duration_since(Instant::now()).as_secs(),"resendAfter":self.resend_at.saturating_duration_since(Instant::now()).as_secs()+u64::from(self.resend_at>Instant::now())})
    }
}
impl Session {
    pub fn new(deployment: Deployment) -> Result<Self, ()> {
        deployment.client()?;
        let _ = key::<32>(&deployment.server_key)?;
        if deployment.captcha_id.len() != 32
            || !deployment.captcha_id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(());
        }
        if deployment.app_key.is_empty() || deployment.device_id.is_empty() {
            return Err(());
        }
        Ok(Self {
            challenge: None,
            saved_login: None,
            vault: None,
            deployment,
            client: None,
            attempts: Vec::new(),
            last_error: None,
            nim: None,
            sync_started: false,
            next_heartbeat: Instant::now(),
            next_profile_sync: Instant::now(),
            reconnect: ReconnectSchedule::default(),
            nim_account: None,
            account_name: None,
            epoch: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            credentials: Arc::new(CredentialCoordinator::default()),
        })
    }
    pub fn view(&self) -> Value {
        let challenge = self
            .challenge
            .as_ref()
            .filter(|c| c.expires > Instant::now())
            .map(SmsChallenge::view);
        let message = self.last_error.as_deref().map(|error| match error {
            "login_rejected_without_challenge" => {
                "登录被拒绝，服务端未提供验证码挑战；请核对账号密码后重试"
            }
            "device_challenge_invalid" => "设备验证返回格式未识别，请联系管理员检查登录协议",
            "device_challenge_network" => "设备验证请求网络失败，请稍后重试",
            "device_challenge_rejected" => {
                "设备验证请求被拒绝；如使用已保存密码，请重新输入当前密码后重试"
            }
            "device_challenge_unavailable" => "设备验证服务返回异常，请稍后重试",
            "password_attempts_exceeded" => "密码尝试次数过多，请切换短信验证码登录",
            "invalid_credentials" => "账号或密码错误，请核对后重试；也可切换短信登录",
            "sms_code_expired" => "短信验证码已过期，请重新获取验证码",
            "sms_send_failed" => "短信发送未成功，请稍后重试；未创建验证会话",
            "password_login_rejected" => {
                "密码登录请求未通过，尚未进入设备短信验证；请联系管理员检查登录链路"
            }
            "sms_expired" => "验证码验证已过期，请返回账号登录后重新获取",
            "sms_failed" => "验证码校验未通过，请检查验证码后重试",
            "sms_resend_failed" => "短信重发失败，请稍后重试",
            "device_verification_failed" => "设备验证短信获取失败，请重新登录后重试",
            _ if self.client.is_some() => "账号已登录，即时通信连接失败",
            _ => "登录校验未通过，请检查账号信息后重试",
        });
        json!({"mode":"rust","configured":true,"authenticated":self.client.is_some(),"nimConnected":self.nim.is_some(),"lastError":self.last_error,"message":message,"challenge":challenge,"captchaId":self.deployment.captcha_id,"savedAccount":self.saved_login.as_ref().map(|v|&v.account)})
    }
    async fn finish_login(&mut self, mut client: Client, data: Value) {
        if let Err(error) = client.groups().await {
            self.last_error = Some(format!("groups_{error:?}"));
            return;
        }
        self.challenge = None;
        self.account_name = ["userNick", "nickname", "nick", "userName"]
            .iter()
            .find_map(|key| {
                data.get(*key)
                    .and_then(Value::as_str)
                    .filter(|v| !v.trim().is_empty())
                    .map(str::to_owned)
            });
        self.nim_account = data
            .get("nimId")
            .filter(|v| v.is_number() || v.is_string())
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            });
        let token = data
            .get("nimToken")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        self.client = Some(client);
        if self.save_session().is_err() {
            self.client = None;
            self.nim_account = None;
            self.last_error = Some("session_save_failed".into());
            return;
        }
        self.reconnect = ReconnectSchedule::default();
        self.last_error = match token {
            Some(token) => self.connect_nim(token).await.err(),
            None => Some("nim_token_missing".into()),
        };
    }
    async fn connect_nim(&mut self, token: String) -> Result<(), String> {
        if let Some(socket) = self.nim.take() {
            let _ = socket.close().await;
        }
        self.credentials
            .login(Secret::new(String::new()), Secret::new(token))
            .await;
        let account = self.nim_account.as_deref().ok_or("nim_identity")?;
        let endpoints = dh_protocol::nim_client::discover(
            account,
            &self.deployment.app_key,
            Duration::from_secs(10),
        )
        .await
        .map_err(|e| format!("nim_discovery_{e:?}"))?;
        let session = uuid::Uuid::new_v4().to_string();
        let fields = [
            ("appKey", self.deployment.app_key.as_str()),
            ("account", account),
            ("deviceId", self.deployment.device_id.as_str()),
            ("session", session.as_str()),
            ("clientType", "16"),
            ("sdkVersion", "92114"),
            ("sdkHumanVersion", "9.21.14"),
            ("protocolVersion", "1"),
            ("appLogin", "1"),
            ("os", "Linux"),
            ("browser", "DH Rust"),
            ("userAgent", "Native/9.21.14"),
            ("sdkType", "0"),
            ("isReactNative", "0"),
            ("customTag", ""),
        ];
        let mut error = "nim_no_endpoint".to_owned();
        for endpoint in endpoints.iter().take(3) {
            match NimClient::connect(
                endpoint,
                &fields,
                self.credentials.clone(),
                Duration::from_secs(10),
            )
            .await
            {
                Ok(mut socket) => {
                    socket
                        .heartbeat(2)
                        .await
                        .map_err(|e| format!("nim_heartbeat_{e:?}"))?;
                    self.nim = Some(socket);
                    self.sync_started = false;
                    self.next_heartbeat = Instant::now() + Duration::from_secs(15);
                    self.reconnect = ReconnectSchedule::default();
                    return Ok(());
                }
                Err(e) => error = format!("nim_auth_{e:?}"),
            }
        }
        Err(error)
    }
    async fn sync_account_name(&mut self) {
        if self.account_name.is_some() || Instant::now() < self.next_profile_sync {
            return;
        }
        self.next_profile_sync = Instant::now() + Duration::from_secs(60);
        let Some(client) = self.client.as_mut() else {
            return;
        };
        let Ok(user) = client.user_id() else {
            return;
        };
        let name = tokio::time::timeout(Duration::from_secs(8), async {
            let data = client.groups().await.ok()?;
            let account = self.nim_account.as_deref()?;
            let groups = crate::business_reads::groups(account, &data).ok()?;
            for group in groups.iter().take(3) {
                let data = client.members_page(group.group_id, None).await.ok()?;
                let (members, _) =
                    crate::business_reads::members(account, group.group_id, &data).ok()?;
                if let Some(member) = members
                    .iter()
                    .find(|m| m.user_id == user && !m.nickname.trim().is_empty())
                {
                    return Some(member.nickname.clone());
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        if let Some(name) = name {
            self.account_name = Some(name);
            let _ = self.save_session();
        }
    }
    pub async fn maintain_connection(&mut self) {
        if self.client.is_none() || self.nim_account.is_none() {
            return;
        }
        self.sync_account_name().await;
        if Instant::now() >= self.next_heartbeat {
            if let Some(socket) = &mut self.nim {
                if let Err(error) =
                    tokio::time::timeout(Duration::from_secs(5), socket.heartbeat(3))
                        .await
                        .unwrap_or(Err(dh_protocol::nim_client::Error::Timeout))
                {
                    self.last_error = Some(format!("nim_heartbeat_{error:?}"));
                    self.nim = None;
                    self.sync_started = false;
                } else {
                    self.next_heartbeat = Instant::now() + Duration::from_secs(15);
                }
            }
        }
        if self.nim.is_none() && self.reconnect.ready(Instant::now()) {
            self.renew_connection().await;
        }
    }

    async fn renew_connection(&mut self) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        self.sync_started = false;
        match client.refresh_nim().await {
            Ok(token) => self.last_error = self.connect_nim(token.to_string()).await.err(),
            Err(error) => {
                if matches!(
                    error,
                    dh_protocol::business_client::Error::Expired
                        | dh_protocol::business_client::Error::Business(401)
                ) {
                    self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    self.client = None;
                    self.nim = None;
                    self.nim_account = None;
                    self.credentials.logout().await;
                    if let Some(vault) = &self.vault {
                        if vault.clear_session().is_err() {
                            self.last_error = Some("session_clear_failed".into());
                            return;
                        }
                    }
                }
                self.last_error = Some(format!("nim_refresh_{error:?}"));
            }
        }
        if self.nim.is_none() {
            self.reconnect.failed(Instant::now());
        }
    }
}
fn requires_device_verification(error: &dh_protocol::business_client::Error) -> bool {
    matches!(error, dh_protocol::business_client::Error::Business(1069))
}
type ResultJson = Result<Json<Value>, StatusCode>;
fn check(server: &Server, headers: &HeaderMap) -> Result<(), StatusCode> {
    if !authorized(server, headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !same_origin(server, headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
pub async fn status(State(server): State<Arc<Server>>, headers: HeaderMap) -> ResultJson {
    check(&server, &headers)?;
    let guard = tokio::time::timeout(Duration::from_secs(2), server.protocol.lock())
        .await
        .map_err(|_| StatusCode::CONFLICT)?;
    Ok(Json(guard.as_ref().map(Session::view).unwrap_or_else(
        || json!({"mode":"cdp","configured":false,"authenticated":false}),
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Login {
    account: String,
    password: String,
    validate_str: String,
    #[serde(default)]
    remember_password: bool,
    #[serde(default)]
    use_saved_password: bool,
}
impl Drop for Login {
    fn drop(&mut self) {
        self.password.zeroize();
        self.validate_str.zeroize();
    }
}
pub async fn login(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    Json(mut input): Json<Login>,
) -> ResultJson {
    check(&server, &headers)?;
    if input.account.is_empty()
        || input.account.len() > 64
        || (input.password.is_empty() && !input.use_saved_password)
        || input.password.len() > 1024
        || input.validate_str.is_empty()
        || input.validate_str.len() > 8192
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut guard = server
        .protocol
        .try_lock()
        .map_err(|_| StatusCode::CONFLICT)?;
    let session = guard.as_mut().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    if input.use_saved_password {
        let saved = session
            .saved_login
            .as_ref()
            .filter(|v| v.account == input.account)
            .ok_or(StatusCode::BAD_REQUEST)?;
        input.password = saved.password.clone();
    }
    session
        .attempts
        .retain(|t| t.elapsed() < Duration::from_secs(60));
    if session.attempts.len() >= 5 {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    session.attempts.push(Instant::now());
    if input.remember_password {
        let vault = session
            .vault
            .as_ref()
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        let saved = SavedLogin {
            account: input.account.clone(),
            password: input.password.clone(),
        };
        let bytes = Zeroizing::new(
            serde_json::to_vec(&saved).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        vault
            .save("login.sealed", &bytes)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        session.saved_login = Some(saved);
    } else if let Some(vault) = &session.vault {
        vault
            .save("login.sealed", b"null")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        session.saved_login = None;
    }
    if let Some(vault) = &session.vault {
        vault
            .clear_session()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    session.challenge = None;
    session
        .epoch
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    session.client = None;
    session.nim = None;
    session.nim_account = None;
    session.credentials.logout().await;
    session.last_error = None;
    let mut client = session
        .deployment
        .client()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let ty = if input.account.len() == 11 && input.account.bytes().all(|b| b.is_ascii_digit()) {
        "LOGIN_TYPE_PHONE_PWD"
    } else {
        "LOGIN_TYPE_ACCOUNT_PWD"
    };
    let mut params = json!({"account":input.account,"passwd":input.password,"validateStr":input.validate_str,"type":ty});
    let result = client
        .login(
            &params,
            *key(&session.deployment.server_key).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?,
        )
        .await;
    for field in ["passwd", "validateStr"] {
        if let Some(Value::String(value)) = params.get_mut(field) {
            value.zeroize();
        }
    }
    match result {
        Ok(data) => {
            session.finish_login(client, data).await;
            Ok(Json(session.view()))
        }
        Err(error) if requires_device_verification(&error) => {
            // Keep transport/rejection/shape failures distinct; never discard them with .ok().
            match client
                .device_verification(&input.account, &input.password, ty)
                .await
            {
                Ok(data) => match SmsChallenge::new(&data) {
                    Ok(challenge) => {
                        session.challenge = Some(challenge);
                        session.last_error = None;
                    }
                    Err(_) => session.last_error = Some("device_challenge_invalid".into()),
                },
                Err(error) => {
                    let category = match error {
                        dh_protocol::business_client::Error::Transport => {
                            "device_challenge_network"
                        }
                        dh_protocol::business_client::Error::Business(_) => {
                            "device_challenge_rejected"
                        }
                        _ => "device_challenge_unavailable",
                    };
                    // Error enums contain only fixed stage labels and numeric codes, not credentials.
                    eprintln!("device_challenge: login_code=1069, error={error:?}");
                    session.last_error = Some(category.into());
                }
            }
            Ok(Json(session.view()))
        }
        Err(error) => {
            eprintln!("password_login: error={error:?}");
            session.last_error = Some(
                match error {
                    dh_protocol::business_client::Error::PasswordAttemptsExceeded => {
                        "password_attempts_exceeded"
                    }
                    dh_protocol::business_client::Error::InvalidCredentials => {
                        "invalid_credentials"
                    }
                    _ => "password_login_rejected",
                }
                .into(),
            );
            Ok(Json(session.view()))
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SmsInput {
    challenge_id: String,
    validate_str: String,
    #[serde(default)]
    verification_code: String,
}
impl Drop for SmsInput {
    fn drop(&mut self) {
        self.validate_str.zeroize();
        self.verification_code.zeroize();
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StartSms {
    phone: String,
    validate_str: String,
}
impl Drop for StartSms {
    fn drop(&mut self) {
        self.phone.zeroize();
        self.validate_str.zeroize();
    }
}
pub async fn start_sms(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    Json(input): Json<StartSms>,
) -> ResultJson {
    check(&server, &headers)?;
    if input.phone.len() != 11
        || !input.phone.bytes().all(|b| b.is_ascii_digit())
        || input.validate_str.is_empty()
        || input.validate_str.len() > 8192
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut guard = server
        .protocol
        .try_lock()
        .map_err(|_| StatusCode::CONFLICT)?;
    let session = guard.as_mut().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    if session.client.is_some() {
        return Err(StatusCode::CONFLICT);
    }
    if session
        .challenge
        .as_ref()
        .is_some_and(|c| c.resend_at > Instant::now())
    {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    // One initial send per minute, including rejected/uncertain requests.
    session
        .attempts
        .retain(|t| t.elapsed() < Duration::from_secs(60));
    if !session.attempts.is_empty() {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    session.attempts.push(Instant::now());
    session.challenge = None;
    let phone = json!({"countryCode":"86","nationalNumber":input.phone,"maskedNationalNumber":format!("{}****{}",&input.phone[..3],&input.phone[7..])});
    let mut client = session
        .deployment
        .client()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    match client
        .resend_device_sms(&phone, "", &input.validate_str)
        .await
    {
        Ok(data) => match SmsChallenge::new(&json!({"phone":phone,"sms":data})) {
            Ok(mut challenge) => {
                challenge.original_key = Zeroizing::new(String::new());
                session.challenge = Some(challenge);
                session.last_error = None;
            }
            Err(_) => session.last_error = Some("device_challenge_invalid".into()),
        },
        Err(error) => {
            eprintln!("sms_start: error={error:?}");
            session.last_error = Some("sms_send_failed".into());
        }
    }
    Ok(Json(session.view()))
}
pub async fn verify_sms(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    Json(input): Json<SmsInput>,
) -> ResultJson {
    sms_action(server, headers, input, false).await
}
pub async fn resend_sms(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    Json(input): Json<SmsInput>,
) -> ResultJson {
    sms_action(server, headers, input, true).await
}
async fn sms_action(
    server: Arc<Server>,
    headers: HeaderMap,
    input: SmsInput,
    resend: bool,
) -> ResultJson {
    check(&server, &headers)?;
    if input.validate_str.is_empty()
        || input.validate_str.len() > 8192
        || (!resend
            && (input.verification_code.len() != 6
                || !input.verification_code.bytes().all(|b| b.is_ascii_digit())))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut guard = server
        .protocol
        .try_lock()
        .map_err(|_| StatusCode::CONFLICT)?;
    let session = guard.as_mut().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let challenge = session.challenge.as_ref().ok_or(StatusCode::CONFLICT)?;
    if challenge.id != input.challenge_id {
        return Err(StatusCode::CONFLICT);
    }
    if challenge.expires <= Instant::now() {
        session.challenge = None;
        session.last_error = Some("sms_expired".into());
        return Ok(Json(session.view()));
    }
    if resend && challenge.resend_at > Instant::now() {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    session
        .attempts
        .retain(|t| t.elapsed() < Duration::from_secs(60));
    if session.attempts.len() >= 5 {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    session.attempts.push(Instant::now());
    let mut client = session
        .deployment
        .client()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if resend {
        let challenge = session.challenge.as_mut().unwrap();
        challenge.resend_at = Instant::now() + Duration::from_secs(60);
        match client
            .resend_device_sms(
                &challenge.phone,
                &challenge.original_key,
                &input.validate_str,
            )
            .await
        {
            Ok(data) => match data
                .get("key")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty() && v.len() <= 8192)
            {
                Some(key) => {
                    challenge.key = Zeroizing::new(key.into());
                    session.last_error = None;
                }
                None => session.last_error = Some("sms_resend_failed".into()),
            },
            Err(_) => session.last_error = Some("sms_resend_failed".into()),
        }
    } else {
        let challenge = session.challenge.as_ref().unwrap();
        let mut params = json!({"phone":challenge.phone,"key":challenge.key.as_str(),"verificationCode":input.verification_code,"validateStr":input.validate_str,"type":"LOGIN_TYPE_SMS"});
        let result = client
            .login(
                &params,
                *key(&session.deployment.server_key)
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?,
            )
            .await;
        for field in ["key", "verificationCode", "validateStr"] {
            if let Some(Value::String(value)) = params.get_mut(field) {
                value.zeroize();
            }
        }
        match result {
            Ok(data) => session.finish_login(client, data).await,
            Err(dh_protocol::business_client::Error::SmsExpired) => {
                session.last_error = Some("sms_code_expired".into())
            }
            Err(dh_protocol::business_client::Error::InvalidSmsCode) => {
                session.last_error = Some("sms_failed".into())
            }
            Err(_) => session.last_error = Some("device_challenge_unavailable".into()),
        }
    }
    Ok(Json(session.view()))
}
pub async fn refresh(State(server): State<Arc<Server>>, headers: HeaderMap) -> ResultJson {
    check(&server, &headers)?;
    let mut guard = server
        .protocol
        .try_lock()
        .map_err(|_| StatusCode::CONFLICT)?;
    let session = guard.as_mut().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    if session.client.is_none() {
        return Err(StatusCode::CONFLICT);
    }
    session.renew_connection().await;
    Ok(Json(session.view()))
}
pub async fn logout(State(server): State<Arc<Server>>, headers: HeaderMap) -> ResultJson {
    check(&server, &headers)?;
    let mut guard = server
        .protocol
        .try_lock()
        .map_err(|_| StatusCode::CONFLICT)?;
    let session = guard.as_mut().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    if let Some(vault) = &session.vault {
        vault
            .clear_session()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    session.challenge = None;
    session
        .epoch
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    session.client = None;
    session.nim = None;
    session.nim_account = None;
    session.credentials.logout().await;
    session.last_error = None;
    Ok(Json(session.view()))
}

pub async fn forget_login(State(server): State<Arc<Server>>, headers: HeaderMap) -> ResultJson {
    check(&server, &headers)?;
    let mut guard = server
        .protocol
        .try_lock()
        .map_err(|_| StatusCode::CONFLICT)?;
    let session = guard.as_mut().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let vault = session
        .vault
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    vault
        .save("login.sealed", b"null")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    session.saved_login = None;
    Ok(Json(session.view()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn status_waits_for_busy_receiver_instead_of_losing_every_poll() {
        let (_dir, server) = crate::tests::setup();
        server.sessions.lock().unwrap().insert(
            crate::admin::digest("test-session"),
            Instant::now() + Duration::from_secs(60),
        );
        let mut headers = HeaderMap::new();
        headers.insert("cookie", "dh_session=test-session".parse().unwrap());
        headers.insert("origin", "http://127.0.0.1:8787".parse().unwrap());
        let guard = server.protocol.lock().await;
        let request = tokio::spawn(status(State(server.clone()), headers));
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(!request.is_finished());
        drop(guard);
        assert!(request.await.unwrap().is_ok());
    }
    #[test]
    fn only_explicit_device_change_enters_sms_flow() {
        use dh_protocol::business_client::Error;
        assert!(requires_device_verification(&Error::Business(1069)));
        for error in [
            Error::Business(1001),
            Error::Business(1040),
            Error::Transport,
            Error::Http(1069),
        ] {
            assert!(!requires_device_verification(&error));
        }
    }
    #[test]
    fn reconnect_is_immediate_then_bounded_and_resettable() {
        let now = Instant::now();
        let mut schedule = ReconnectSchedule::default();
        assert!(schedule.ready(now));
        for delay in [1, 2, 4, 8, 16, 30, 30] {
            schedule.failed(now);
            assert!(!schedule.ready(now + Duration::from_millis(delay * 1000 - 1)));
            assert!(schedule.ready(now + Duration::from_secs(delay)));
        }
        schedule = ReconnectSchedule::default();
        assert!(schedule.ready(now));
        schedule.failed(now);
        assert!(schedule.ready(now + Duration::from_secs(1)));
    }

    #[tokio::test]
    async fn logged_out_session_does_not_attempt_reconnect() {
        let mut session = Session::new(Deployment {
            origin: "https://example.invalid".into(),
            headers: BTreeMap::new(),
            metadata: [0; 14],
            signing_seed: STANDARD.encode([7; 32]),
            body_key: STANDARD.encode([8; 32]),
            message_key: STANDARD.encode([7; 32]),
            server_key: STANDARD.encode([9; 32]),
            captcha_id: "a".repeat(32),
            app_key: "synthetic-app".into(),
            device_id: "synthetic-device".into(),
        })
        .unwrap();
        session.maintain_connection().await;
        assert!(session.last_error.is_none());
        assert_eq!(session.reconnect.failures, 0);
        assert!(session.nim.is_none());
        let dir = tempfile::tempdir().unwrap();
        let vault = crate::vault::Vault::open(dir.path()).unwrap();
        let saved = SavedLogin {
            account: "test-account".into(),
            password: "synthetic-private-password".into(),
        };
        vault
            .save("login.sealed", &serde_json::to_vec(&saved).unwrap())
            .unwrap();
        session.attach_vault(vault).unwrap();
        assert_eq!(session.view()["savedAccount"], "test-account");
        assert!(!session
            .view()
            .to_string()
            .contains("synthetic-private-password"));
        assert_eq!(
            session.saved_login.as_ref().unwrap().password,
            "synthetic-private-password"
        );
    }
    fn challenge() -> SmsChallenge {
        SmsChallenge::new(&json!({"phone":{"countryCode":"86","nationalNumber":"15555551234","maskedNationalNumber":"155****1234"},"sms":{"key":"synthetic-private-sms-key"}})).unwrap()
    }
    #[test]
    fn challenge_view_never_exposes_phone_or_sms_keys() {
        let challenge = challenge();
        let view = challenge.view().to_string();
        assert!(!view.contains("15555551234"));
        assert!(!view.contains("synthetic-private-sms-key"));
        assert!(view.contains("****1234"));
        assert!(SmsChallenge::new(&json!({"phone":{},"sms":{"key":"x"}})).is_err());
        assert!(
            SmsChallenge::new(&json!({"phone":{"nationalNumber":"15555551234"},"sms":{}})).is_err()
        );
    }
    #[tokio::test]
    async fn stale_expired_and_rate_limited_sms_do_not_reach_upstream() {
        let (_dir, server) = crate::tests::setup();
        let session = Session::new(Deployment {
            origin: "https://example.invalid".into(),
            headers: BTreeMap::new(),
            metadata: [0; 14],
            signing_seed: STANDARD.encode([7; 32]),
            body_key: STANDARD.encode([8; 32]),
            message_key: STANDARD.encode([7; 32]),
            server_key: STANDARD.encode([9; 32]),
            captcha_id: "a".repeat(32),
            app_key: "synthetic-app".into(),
            device_id: "synthetic-device".into(),
        })
        .unwrap();
        *server.protocol.lock().await = Some(session);
        server.sessions.lock().unwrap().insert(
            crate::admin::digest("synthetic-session"),
            Instant::now() + Duration::from_secs(60),
        );
        let mut headers = HeaderMap::new();
        headers.insert("origin", "http://127.0.0.1:8787".parse().unwrap());
        headers.insert("cookie", "dh_session=synthetic-session".parse().unwrap());
        let current = challenge();
        let id = current.id.clone();
        server.protocol.lock().await.as_mut().unwrap().challenge = Some(current);
        let input = |id: String| SmsInput {
            challenge_id: id,
            validate_str: "synthetic-human-result".into(),
            verification_code: "123456".into(),
        };
        assert_eq!(
            sms_action(
                server.clone(),
                headers.clone(),
                input("stale".into()),
                false
            )
            .await
            .unwrap_err(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            sms_action(server.clone(), headers.clone(), input(id.clone()), true)
                .await
                .unwrap_err(),
            StatusCode::TOO_MANY_REQUESTS
        );
        server
            .protocol
            .lock()
            .await
            .as_mut()
            .unwrap()
            .challenge
            .as_mut()
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        let value = sms_action(server.clone(), headers.clone(), input(id), false)
            .await
            .unwrap()
            .0;
        assert!(value["challenge"].is_null());
        assert_eq!(value["lastError"], "sms_expired");
        assert!(!value["authenticated"].as_bool().unwrap());
        server.protocol.lock().await.as_mut().unwrap().challenge = Some(challenge());
        let _ = logout(State(server.clone()), headers).await.unwrap();
        assert!(server
            .protocol
            .lock()
            .await
            .as_ref()
            .unwrap()
            .challenge
            .is_none());
    }
}
