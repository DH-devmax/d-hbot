//! Single-administrator lifecycle. Secrets never enter audit records.
use argon2::{
    password_hash::SaltString, Algorithm, Argon2, Params, PasswordHash, PasswordHasher,
    PasswordVerifier, Version,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rand_core::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroize;

pub struct Admin {
    db: Mutex<Connection>,
    legacy: PathBuf,
    pub hashing: Arc<tokio::sync::Semaphore>,
    dummy: String,
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn digest(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
pub fn token() -> String {
    let mut b = [0; 32];
    rand_core::OsRng.fill_bytes(&mut b);
    b.iter().map(|b| format!("{b:02x}")).collect()
}
fn algorithm() -> Argon2<'static> {
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(19456, 2, 1, None).unwrap(),
    )
}
fn hash(s: &str) -> Result<String, StatusCode> {
    algorithm()
        .hash_password(s.as_bytes(), &SaltString::generate(&mut rand_core::OsRng))
        .map(|v| v.to_string())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
fn verify(s: &str, h: &str) -> bool {
    PasswordHash::new(h).is_ok_and(|h| algorithm().verify_password(s.as_bytes(), &h).is_ok())
}
fn username(s: &str) -> Result<String, StatusCode> {
    if !(3..=32).contains(&s.len())
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(s.to_ascii_lowercase())
}
fn password(s: &str) -> Result<(), StatusCode> {
    let n = s.chars().count();
    let lower = s.to_lowercase();
    if !(15..=128).contains(&n)
        || s.chars().all(|c| s.starts_with(c))
        || [
            "123456789012345",
            "password123456789",
            "qwertyuiop123456",
            "123123123123123",
        ]
        .contains(&lower.as_str())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}
impl Admin {
    pub fn open(root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let path = root.join("admin.db");
        let db = Connection::open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        db.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE IF NOT EXISTS admin(id INTEGER PRIMARY KEY CHECK(id=1),username TEXT NOT NULL,hash TEXT NOT NULL,failures INTEGER NOT NULL DEFAULT 0,blocked INTEGER NOT NULL DEFAULT 0); CREATE TABLE IF NOT EXISTS activation(id INTEGER PRIMARY KEY CHECK(id=1),kind TEXT NOT NULL,digest TEXT NOT NULL,expires INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS auth_audit(id INTEGER PRIMARY KEY,event TEXT NOT NULL,at INTEGER NOT NULL);")?;
        Ok(Self {
            db: Mutex::new(db),
            legacy: root.join("admin-password.hash"),
            hashing: Arc::new(tokio::sync::Semaphore::new(2)),
            dummy: hash("dummy-unusable-password-value")
                .map_err(|_| "Hash initialization failed")?,
        })
    }
    pub fn ready(&self) -> bool {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT EXISTS(SELECT 1 FROM admin)", [], |r| r.get(0))
            .unwrap_or(false)
    }
    pub fn mode(&self) -> &'static str {
        if self.ready() {
            "login"
        } else if self.legacy.exists() {
            "migrate"
        } else {
            "activate"
        }
    }
    pub fn issue(&self, reset: bool) -> Result<String, &'static str> {
        if reset != self.ready() {
            return Err("Use activation before initialization; reset after initialization");
        };
        let value = token();
        self.db
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO activation VALUES(1,?,?,?)",
                params![
                    if reset { "reset" } else { "activate" },
                    digest(&value),
                    now() + 1800
                ],
            )
            .map_err(|_| "Cannot create one-time code")?;
        Ok(value)
    }
    pub fn audit(&self, event: &str) {
        let _ = self.db.lock().unwrap().execute(
            "INSERT INTO auth_audit(event,at) VALUES(?,?)",
            params![event, now()],
        );
    }
    pub fn check(&self, name: &str, pass: &str) -> Result<bool, StatusCode> {
        let db = self.db.lock().unwrap();
        let row = db
            .query_row(
                "SELECT username,hash,failures,blocked FROM admin WHERE id=1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u32>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let Some((user, h, failures, blocked)) = row else {
            verify(pass, &self.dummy);
            return Ok(false);
        };
        if blocked > now() {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let matched = user == name.to_ascii_lowercase();
        let valid = verify(pass, if matched { &h } else { &self.dummy }) && matched;
        if valid {
            db.execute("UPDATE admin SET failures=0,blocked=0", [])
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        } else if matched {
            let f = failures.saturating_add(1);
            let delay = if f < 5 {
                0
            } else {
                (30i64 << f.saturating_sub(5).min(5)).min(900)
            };
            db.execute(
                "UPDATE admin SET failures=?,blocked=?",
                params![f, now() + delay],
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        Ok(valid)
    }
    fn change(&self, kind: &str, input: &Input) -> Result<(), StatusCode> {
        let user = username(&input.username)?;
        password(&input.password)?;
        let h = hash(&input.password)?;
        if kind == "migrate" {
            let bytes =
                crate::vault::read_private(&self.legacy).map_err(|_| StatusCode::FORBIDDEN)?;
            if !verify(
                &input.current_password,
                std::str::from_utf8(&bytes).map_err(|_| StatusCode::FORBIDDEN)?,
            ) {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        if kind == "credentials" {
            let name = self
                .db
                .lock()
                .unwrap()
                .query_row("SELECT username FROM admin", [], |r| r.get::<_, String>(0))
                .map_err(|_| StatusCode::FORBIDDEN)?;
            if !self.check(&name, &input.current_password)? {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        let mut db = self.db.lock().unwrap();
        let tx = db
            .transaction()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if kind == "credentials" {
            let current = tx
                .query_row("SELECT hash FROM admin WHERE id=1", [], |r| {
                    r.get::<_, String>(0)
                })
                .map_err(|_| StatusCode::FORBIDDEN)?;
            if !verify(&input.current_password, &current) {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        let exists: bool = tx
            .query_row("SELECT EXISTS(SELECT 1 FROM admin)", [], |r| r.get(0))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if matches!(kind, "activate" | "migrate") && exists {
            return Err(StatusCode::CONFLICT);
        }
        if matches!(kind, "reset" | "credentials") && !exists {
            return Err(StatusCode::CONFLICT);
        }
        if matches!(kind, "reset" | "activate") {
            let n = tx
                .execute(
                    "DELETE FROM activation WHERE id=1 AND kind=? AND digest=? AND expires>?",
                    params![kind, digest(&input.code), now()],
                )
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if n != 1 {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO admin(id,username,hash) VALUES(1,?,?)",
            params![user, h],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tx.execute("DELETE FROM activation", [])
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tx.execute(
            "INSERT INTO auth_audit(event,at) VALUES(?,?)",
            params![kind, now()],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tx.commit().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if self.legacy.exists() {
            let _ = std::fs::remove_file(&self.legacy);
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    username: String,
    password: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    current_password: String,
}
impl Drop for Input {
    fn drop(&mut self) {
        self.password.zeroize();
        self.code.zeroize();
        self.current_password.zeroize();
    }
}
pub async fn status(State(s): State<Arc<crate::Server>>) -> Json<serde_json::Value> {
    Json(json!({"mode":s.admin.mode()}))
}
async fn update(
    s: Arc<crate::Server>,
    headers: HeaderMap,
    input: Input,
    kind: &'static str,
) -> Response {
    if !crate::same_origin(&s, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if kind == "credentials" && !crate::authorized(&s, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if input.password.len() > 512 || input.current_password.len() > 512 || input.code.len() > 128 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(permit) = s.admin.hashing.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let admin = s.admin.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        admin.change(kind, &input)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            let mut sessions = s.sessions.lock().unwrap();
            sessions.clear();
            s.revocation.send_modify(|n| *n += 1);
            s.activity.lock().unwrap().clear();
            Json(json!({"ok":true})).into_response()
        }
        Ok(Err(code)) => code.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
macro_rules! endpoint {
    ($name:ident,$kind:literal) => {
        pub async fn $name(
            State(s): State<Arc<crate::Server>>,
            headers: HeaderMap,
            Json(input): Json<Input>,
        ) -> Response {
            update(s, headers, input, $kind).await
        }
    };
}
endpoint!(activate, "activate");
endpoint!(migrate, "migrate");
endpoint!(reset, "reset");
endpoint!(credentials, "credentials");
#[cfg(test)]
impl Admin {
    pub fn seed_test(&self, h: &str) {
        self.db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO admin(id,username,hash) VALUES(1,'admin',?)",
                [h],
            )
            .unwrap();
    }
}

#[derive(Default)]
pub struct Limits {
    buckets: std::collections::HashMap<(String, String), Vec<std::time::Instant>>,
}
impl Limits {
    fn allow(&mut self, route: &str, ip: &str) -> bool {
        let now = std::time::Instant::now();
        self.buckets.retain(|_, v| {
            v.retain(|t| now.duration_since(*t).as_secs() < 60);
            !v.is_empty()
        });
        let keys = [
            (route.to_owned(), "global".into()),
            (route.to_owned(), ip.to_owned()),
        ];
        if self.buckets.len() > 4096 {
            return false;
        }
        if keys
            .iter()
            .zip([30, 5])
            .any(|(key, max)| self.buckets.get(key).is_some_and(|v| v.len() >= max))
        {
            return false;
        }
        for key in keys {
            self.buckets.entry(key).or_default().push(now);
        }
        true
    }
}
// Match the router's literal URI paths; unknown paths must not allocate auth buckets.
fn auth_route(path: &str) -> Option<&'static str> {
    match path {
        "/api/login" => Some("/api/login"),
        "/api/setup/activate" => Some("/api/setup/activate"),
        "/api/setup/migrate" => Some("/api/setup/migrate"),
        "/api/admin/reset" => Some("/api/admin/reset"),
        "/api/admin/credentials" => Some("/api/admin/credentials"),
        _ => None,
    }
}
pub async fn guard(
    State(s): State<Arc<crate::Server>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_owned();
    let sensitive =
        path == "/api/login" || path.starts_with("/api/setup/") || path.starts_with("/api/admin/");
    let route = auth_route(&path).filter(|_| req.method() == axum::http::Method::POST);
    let mut response = if let Some(route) = route {
        let peer = req
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|v| v.0.ip());
        let ip = peer
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into());
        let ip = if peer.is_some_and(|p| s.trusted_proxies.contains(&p)) {
            req.headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<std::net::IpAddr>().ok())
                .map(|v| v.to_string())
                .unwrap_or(ip)
        } else {
            ip
        };
        if !s.limits.lock().unwrap().allow(route, &ip) {
            s.admin.audit("rate_limited");
            StatusCode::TOO_MANY_REQUESTS.into_response()
        } else {
            let (parts, body) = req.into_parts();
            match axum::body::to_bytes(body, 16384).await {
                Ok(bytes) => {
                    req = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
                    next.run(req).await
                }
                Err(_) => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
            }
        }
    } else {
        next.run(req).await
    };
    let headers = response.headers_mut();
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("content-security-policy-report-only","default-src 'self'; script-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'self'".parse().unwrap());
    if sensitive {
        headers.insert("cache-control", "no-store".parse().unwrap());
    }
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert("retry-after", "60".parse().unwrap());
    }
    response
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input(code: String) -> Input {
        Input {
            username: "Owner".into(),
            password: "A distinct test passphrase!".into(),
            code,
            current_password: String::new(),
        }
    }
    #[test]
    fn activation_rotation_replay_reset_and_throttle_persist() {
        let dir = tempfile::tempdir().unwrap();
        let a = Admin::open(dir.path()).unwrap();
        let first = a.issue(false).unwrap();
        let second = a.issue(false).unwrap();
        assert_eq!(
            a.change("activate", &input(first)),
            Err(StatusCode::UNAUTHORIZED)
        );
        a.change("activate", &input(second.clone())).unwrap();
        assert_eq!(a.mode(), "login");
        assert_eq!(
            a.change("activate", &input(second)),
            Err(StatusCode::CONFLICT)
        );
        assert!(a.check("OWNER", "A distinct test passphrase!").unwrap());
        for _ in 0..5 {
            assert!(!a.check("owner", "bad").unwrap());
        }
        drop(a);
        let a = Admin::open(dir.path()).unwrap();
        assert_eq!(
            a.check("owner", "A distinct test passphrase!"),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        let reset = a.issue(true).unwrap();
        a.change("reset", &input(reset)).unwrap();
        assert!(a.check("owner", "A distinct test passphrase!").unwrap());
    }
    #[test]
    fn only_registered_auth_routes_allocate_buckets() {
        let mut limits = Limits::default();
        for path in [
            "/api/login",
            "/api/setup/activate",
            "/api/setup/migrate",
            "/api/admin/reset",
            "/api/admin/credentials",
        ] {
            let route = auth_route(path).unwrap();
            for _ in 0..5 {
                assert!(limits.allow(route, "one-client"));
            }
            assert!(!limits.allow(route, "one-client"));
            assert!(limits.allow(route, "another-client"));
        }
        for path in [
            "/api/setup/status",
            "/api/admin/reset/",
            "/api/admin/%72eset",
            "/api/admin/reset/extra",
            "/API/login",
            "/api/setup/../login",
        ] {
            assert!(auth_route(path).is_none(), "{path}");
        }
    }

    #[test]
    fn expiry_weak_password_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let a = Admin::open(dir.path()).unwrap();
        let code = a.issue(false).unwrap();
        a.db.lock()
            .unwrap()
            .execute("UPDATE activation SET expires=0", [])
            .unwrap();
        assert_eq!(
            a.change("activate", &input(code)),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert!(password("123123123123123").is_err());
        assert!(username("a b").is_err());
        let mut l = Limits::default();
        for _ in 0..5 {
            assert!(l.allow("login", "1"));
        }
        assert!(!l.allow("login", "1"));
        assert!(l.allow("login", "2"));
        assert!(l.allow("reset", "1"));
    }
}
#[cfg(test)]
mod integration_tests {
    use super::*;
    use axum::{
        body::Body,
        http::Request,
        routing::{get, post},
        Router,
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn unknown_auth_paths_cannot_starve_real_login() {
        let (_dir, s) = crate::tests::setup();
        let app = Router::new()
            .route("/api/login", post(crate::login))
            .fallback(|| async { StatusCode::NOT_FOUND })
            .layer(axum::middleware::from_fn_with_state(s.clone(), guard))
            .with_state(s.clone());
        let request = |path: String, body: &str| {
            Request::builder()
                .method("POST")
                .uri(path)
                .header("origin", "http://127.0.0.1:8787")
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        for i in 0..2100 {
            let path = if i % 2 == 0 {
                format!("/api/setup/unknown-{i}")
            } else {
                format!("/api/admin/%72eset-{i}")
            };
            let response = app.clone().oneshot(request(path, "{}")).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        assert!(s.limits.lock().unwrap().buckets.is_empty());
        let body = r#"{"username":"admin","password":"synthetic-test-password"}"#;
        for _ in 0..5 {
            let response = app
                .clone()
                .oneshot(request("/api/login?source=test".into(), body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = app
            .oneshot(request("/api/login?source=test".into(), body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "60");
    }

    #[tokio::test]
    async fn guarded_routes_enforce_limits_size_and_origin() {
        let (_dir, s) = crate::tests::setup();
        let app = Router::new()
            .route("/api/setup/status", get(status))
            .route("/api/setup/activate", post(activate))
            .layer(axum::middleware::from_fn_with_state(s.clone(), guard))
            .with_state(s);
        let request = |body: String, origin: &str| {
            Request::builder()
                .method("POST")
                .uri("/api/setup/activate")
                .header("origin", origin)
                .header("content-type", "application/json")
                .header("x-real-ip", token())
                .body(Body::from(body))
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request("x".repeat(16385), "http://127.0.0.1:8787"))
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let body=serde_json::json!({"username":"owner","password":"A distinct passphrase!","code":"bad"}).to_string();
        assert_eq!(
            app.clone()
                .oneshot(request(body.clone(), "https://untrusted.invalid"))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        for _ in 0..3 {
            assert_eq!(
                app.clone()
                    .oneshot(request(body.clone(), "http://127.0.0.1:8787"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CONFLICT
            );
        }
        let response = app
            .oneshot(request(body, "http://127.0.0.1:8787"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "60");
    }
    #[test]
    fn only_one_concurrent_activation_wins() {
        let dir = tempfile::tempdir().unwrap();
        let a = Arc::new(Admin::open(dir.path()).unwrap());
        let code = a.issue(false).unwrap();
        let workers = (0..2)
            .map(|_| {
                let a = a.clone();
                let code = code.clone();
                std::thread::spawn(move || {
                    a.change(
                        "activate",
                        &Input {
                            username: "owner".into(),
                            password: "Concurrent test password!".into(),
                            code,
                            current_password: String::new(),
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        let outcomes = workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    }
}
#[cfg(test)]
mod session_tests {
    use super::*;
    #[test]
    fn idle_absolute_expiry_and_digest_storage() {
        let (_dir, s) = crate::tests::setup();
        let raw = token();
        let key = digest(&raw);
        let mut headers = HeaderMap::new();
        headers.insert("cookie", format!("dh_session={raw}").parse().unwrap());
        s.sessions.lock().unwrap().insert(
            key.clone(),
            std::time::Instant::now() + std::time::Duration::from_secs(3600),
        );
        s.activity
            .lock()
            .unwrap()
            .insert(key.clone(), std::time::Instant::now());
        assert!(crate::authorized(&s, &headers));
        assert!(!s.sessions.lock().unwrap().contains_key(&raw));
        s.activity.lock().unwrap().insert(
            key.clone(),
            std::time::Instant::now() - std::time::Duration::from_secs(1801),
        );
        assert!(!crate::authorized(&s, &headers));
        s.activity
            .lock()
            .unwrap()
            .insert(key.clone(), std::time::Instant::now());
        s.sessions.lock().unwrap().insert(
            key,
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        );
        assert!(!crate::authorized(&s, &headers));
    }
    #[tokio::test]
    async fn hash_work_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let a = Admin::open(dir.path()).unwrap();
        let _one = a.hashing.try_acquire().unwrap();
        let _two = a.hashing.try_acquire().unwrap();
        assert!(a.hashing.try_acquire().is_err());
    }
}
