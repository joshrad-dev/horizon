//! Microsoft account sign-in for the launcher the game shows itself (`launcher_ui`, the iOS
//! app's), as the desktop launcher signs in (launcher/src/account.rs, docs/microsoft-account.md):
//! the OAuth device-code flow, then Xbox Live, XSTS and the Minecraft services, the ownership
//! check and the profile. The Azure app is Horizon's own (`HORIZON_MSA_CLIENT_ID`, at build time
//! or at run time); without one there is no sign-in.
//!
//! What has to be kept (the Microsoft refresh token, the Minecraft token and when it ends) goes
//! to a [`SecretStore`] the platform installs (the iOS Keychain), else to a file only the player
//! can read. Tokens never reach logs or errors.
use crate::{auth::AuthSession, lang};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCOPE: &str = "XboxLive.signin offline_access";
const MICROSOFT: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const XBOX_USER: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MINECRAFT: &str = "https://api.minecraftservices.com";
/// The Minecraft token is renewed when it has less than this left.
const RENEW_BEFORE: u64 = 3600;

/// The configured Azure app, if any: the run-time variable wins over the build-time one, and an
/// empty value means none.
pub fn client_id() -> Option<String> {
    std::env::var("HORIZON_MSA_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("HORIZON_MSA_CLIENT_ID").map(str::to_owned))
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
}

/// Where the account's secrets are kept: one value, the JSON of [`Secrets`].
pub trait SecretStore: Send + Sync {
    fn get(&self) -> Option<String>;
    fn set(&self, value: &str) -> Result<(), String>;
    fn delete(&self);
}

static STORE: OnceLock<&'static dyn SecretStore> = OnceLock::new();

/// Installs the platform's store (the iOS Keychain). Only the first call counts.
pub fn install_store(store: &'static dyn SecretStore) {
    let _ = STORE.set(store);
}

/// The file the secrets go to without a platform store: in the game's home, the player's alone.
struct FileStore;
impl FileStore {
    fn path() -> std::path::PathBuf {
        crate::logging::game_home().join("account-secrets.json")
    }
}
impl SecretStore for FileStore {
    fn get(&self) -> Option<String> {
        std::fs::read_to_string(Self::path()).ok()
    }
    fn set(&self, value: &str) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, value.as_bytes()).map_err(|e| e.to_string())
    }
    fn delete(&self) {
        let _ = std::fs::remove_file(Self::path());
    }
}

fn store() -> &'static dyn SecretStore {
    STORE.get().copied().unwrap_or(&FileStore)
}

/// What is kept of a signed-in account.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Secrets {
    name: String,
    /// The profile UUID without dashes.
    uuid: String,
    refresh_token: String,
    minecraft_token: String,
    minecraft_expires_at: u64,
    #[serde(default)]
    xuid: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(&format!("Horizon/{}", horizon_core::release::VERSION))
        .build()
}

/// A request's status and JSON body, whatever the status; an error only when nothing came back.
fn send(request: ureq::Request, body: Option<Body>) -> Result<(u16, Value), String> {
    let response = match body {
        None => request.call(),
        Some(Body::Form(fields)) => request.send_form(&fields),
        Some(Body::Json(value)) => request
            .set("Content-Type", "application/json")
            .set("Accept", "application/json")
            .send_string(&value.to_string()),
    };
    let response = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(_)) => return Err(lang::get("horizon.msa.offline")),
    };
    let status = response.status();
    let body = response
        .into_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    Ok((status, body))
}

enum Body<'a> {
    Form(Vec<(&'a str, &'a str)>),
    Json(Value),
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// The OAuth error codes as messages for the player.
fn oauth_error(v: &Value) -> String {
    let key = match v.get("error").and_then(Value::as_str) {
        Some("authorization_declined") => "horizon.msa.declined",
        Some("expired_token") => "horizon.msa.expired_code",
        Some("bad_verification_code") => "horizon.msa.bad_code",
        Some("invalid_grant") => "horizon.msa.sign_in_again",
        Some("invalid_client" | "unauthorized_client") => "horizon.msa.bad_client",
        _ => "horizon.msa.failed",
    };
    lang::get(key)
}

/// The XSTS `XErr` codes, as the official launcher explains them.
fn xsts_error(code: u64) -> String {
    match code {
        2148916233 => lang::get("horizon.msa.xbox.no_profile"),
        2148916235 => lang::get("horizon.msa.xbox.region"),
        2148916236 | 2148916237 => lang::get("horizon.msa.xbox.adult"),
        2148916238 => lang::get("horizon.msa.xbox.child"),
        other => lang::get("horizon.msa.xbox.refused").replace("%s", &other.to_string()),
    }
}

/// What the player types at the verification page.
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    interval: u64,
    expires_at: u64,
    device_code: String,
}

fn device_code(client_id: &str) -> Result<DeviceCode, String> {
    let (status, v) = send(
        agent().post(&format!("{MICROSOFT}/devicecode")),
        Some(Body::Form(vec![("client_id", client_id), ("scope", SCOPE)])),
    )?;
    match (
        status,
        str_field(&v, "user_code"),
        str_field(&v, "device_code"),
    ) {
        (200, Some(user_code), Some(device_code)) => Ok(DeviceCode {
            user_code,
            verification_uri: str_field(&v, "verification_uri")
                .unwrap_or_else(|| "https://www.microsoft.com/link".into()),
            interval: v
                .get("interval")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .clamp(1, 60),
            expires_at: now().saturating_add(
                v.get("expires_in")
                    .and_then(Value::as_u64)
                    .unwrap_or(900)
                    .min(3600),
            ),
            device_code,
        }),
        _ => Err(oauth_error(&v)),
    }
}

/// The Microsoft access and refresh tokens.
struct MsTokens {
    access: String,
    refresh: String,
}

fn ms_tokens(status: u16, v: &Value) -> Result<MsTokens, String> {
    match (
        status,
        str_field(v, "access_token"),
        str_field(v, "refresh_token"),
    ) {
        (200, Some(access), Some(refresh)) => Ok(MsTokens { access, refresh }),
        _ => Err(oauth_error(v)),
    }
}

/// Polls until the player signed in on the page, the code expired, or `cancelled`.
fn wait_for_sign_in(
    client_id: &str,
    code: &DeviceCode,
    cancelled: &AtomicBool,
) -> Result<MsTokens, String> {
    let mut interval = code.interval;
    loop {
        let until = std::time::Instant::now() + Duration::from_secs(interval);
        while std::time::Instant::now() < until {
            if cancelled.load(Ordering::Relaxed) {
                return Err(lang::get("horizon.msa.cancelled"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if now() > code.expires_at {
            return Err(lang::get("horizon.msa.expired_code"));
        }
        let (status, v) = send(
            agent().post(&format!("{MICROSOFT}/token")),
            Some(Body::Form(vec![
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client_id),
                ("device_code", &code.device_code),
            ])),
        )?;
        match v.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = (interval + 5).min(60),
            _ => return ms_tokens(status, &v),
        }
    }
}

fn refresh_ms(client_id: &str, refresh: &str) -> Result<MsTokens, String> {
    let (status, v) = send(
        agent().post(&format!("{MICROSOFT}/token")),
        Some(Body::Form(vec![
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh),
            ("scope", SCOPE),
        ])),
    )?;
    ms_tokens(status, &v)
}

/// The Minecraft token, its end and the XUID, from the Microsoft access token.
fn minecraft_login(ms_access: &str) -> Result<(String, u64, Option<String>), String> {
    let (status, xbl) = send(
        agent().post(XBOX_USER),
        Some(Body::Json(json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))),
    )?;
    let xbl_token = str_field(&xbl, "Token")
        .filter(|_| status == 200)
        .ok_or_else(|| lang::get("horizon.msa.xbox.refused").replace("%s", &status.to_string()))?;
    let (status, xsts) = send(
        agent().post(XSTS),
        Some(Body::Json(json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }))),
    )?;
    if let Some(code) = xsts.get("XErr").and_then(Value::as_u64) {
        return Err(xsts_error(code));
    }
    let claims = xsts.pointer("/DisplayClaims/xui/0");
    let (Some(xsts_token), Some(uhs)) = (
        str_field(&xsts, "Token").filter(|_| status == 200),
        claims.and_then(|c| str_field(c, "uhs")),
    ) else {
        return Err(lang::get("horizon.msa.xbox.refused").replace("%s", &status.to_string()));
    };
    let xuid = claims.and_then(|c| str_field(c, "xid"));
    let xtoken = format!("XBL3.0 x={uhs};{xsts_token}");
    let lifetime = |v: &Value| v.get("expires_in").and_then(Value::as_u64).unwrap_or(86400);
    // `launcher/login` first (a new account's `login_with_xbox` token names no profile), as the
    // desktop launcher does.
    if let Ok((200, v)) = send(
        agent().post(&format!("{MINECRAFT}/launcher/login")),
        Some(Body::Json(
            json!({ "xtoken": xtoken, "platform": "PC_LAUNCHER" }),
        )),
    ) && let Some(token) = str_field(&v, "access_token")
    {
        return Ok((token, now() + lifetime(&v).min(7 * 86_400), xuid));
    }
    let (status, mc) = send(
        agent().post(&format!("{MINECRAFT}/authentication/login_with_xbox")),
        Some(Body::Json(json!({ "identityToken": xtoken }))),
    )?;
    match (status, str_field(&mc, "access_token")) {
        (200, Some(token)) => Ok((token, now() + lifetime(&mc).min(7 * 86_400), xuid)),
        (403, _) => Err(lang::get("horizon.msa.not_approved")),
        (429, _) => Err(lang::get("horizon.msa.too_many")),
        _ => Err(lang::get("horizon.msa.failed")),
    }
}

/// The profile's UUID (no dashes) and name, once the account owns the game and has one.
fn owned_profile(token: &str) -> Result<(String, String), String> {
    let bearer = format!("Bearer {token}");
    let (status, v) = send(
        agent()
            .get(&format!("{MINECRAFT}/entitlements/mcstore"))
            .set("Authorization", &bearer),
        None,
    )?;
    let owns = status == 200
        && v.get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|i| {
                    matches!(
                        i.get("name").and_then(Value::as_str),
                        Some("game_minecraft" | "product_minecraft")
                    )
                })
            });
    if !owns {
        return Err(lang::get("horizon.msa.not_owned"));
    }
    let (status, v) = send(
        agent()
            .get(&format!("{MINECRAFT}/minecraft/profile"))
            .set("Authorization", &bearer),
        None,
    )?;
    match (status, str_field(&v, "id"), str_field(&v, "name")) {
        (200, Some(id), Some(name)) => Ok((id.replace('-', "").to_lowercase(), name)),
        _ => Err(lang::get("horizon.msa.no_profile")),
    }
}

/// From the Microsoft tokens to a session, and what to keep of it.
fn finish(ms: MsTokens) -> Result<(AuthSession, Secrets), String> {
    let (token, expires_at, xuid) = minecraft_login(&ms.access)?;
    let (uuid, name) = owned_profile(&token)?;
    let secrets = Secrets {
        name,
        uuid,
        refresh_token: ms.refresh,
        minecraft_token: token,
        minecraft_expires_at: expires_at,
        xuid,
    };
    Ok((session_of(&secrets)?, secrets))
}

fn session_of(secrets: &Secrets) -> Result<AuthSession, String> {
    AuthSession::parse(
        &json!({
            "name": secrets.name,
            "uuid": secrets.uuid,
            "access_token": secrets.minecraft_token,
            "user_type": "msa",
            "xuid": secrets.xuid,
        })
        .to_string(),
    )
}

fn save(secrets: &Secrets) -> Result<(), String> {
    let text = serde_json::to_string(secrets).map_err(|e| e.to_string())?;
    store().set(&text)
}

/// The account signed in before, if any: its name (no network).
pub fn remembered() -> Option<String> {
    let secrets: Secrets = serde_json::from_str(&store().get()?).ok()?;
    Some(secrets.name)
}

/// Forget the account.
pub fn sign_out() {
    store().delete();
}

/// The remembered account's session, its Minecraft token renewed when it is near its end:
/// `None` without one, an error when it can't be renewed (the account must sign in again).
pub fn restore() -> Result<Option<AuthSession>, String> {
    let Some(text) = store().get() else {
        return Ok(None);
    };
    let Ok(secrets) = serde_json::from_str::<Secrets>(&text) else {
        store().delete();
        return Ok(None);
    };
    if secrets.minecraft_expires_at > now() + RENEW_BEFORE {
        return session_of(&secrets).map(Some);
    }
    let client_id = client_id().ok_or_else(|| lang::get("horizon.msa.bad_client"))?;
    let ms = refresh_ms(&client_id, &secrets.refresh_token)?;
    let (session, renewed) = finish(ms)?;
    save(&renewed)?;
    Ok(Some(session))
}

/// Where a sign-in is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    Requesting,
    /// The code to type, and where.
    Code {
        code: String,
        url: String,
    },
    /// Signed in at Microsoft: on to Xbox Live and Minecraft.
    Finishing,
    /// Signed in as this name; the session is [`SignIn::take_session`]'s.
    Done(String),
    Failed(String),
}

/// A sign-in on a thread of its own.
pub struct SignIn {
    progress: mpsc::Receiver<Progress>,
    session: Arc<Mutex<Option<AuthSession>>>,
    cancelled: Arc<AtomicBool>,
    pub last: Progress,
}

impl SignIn {
    /// Start one with the configured app; `None` without one.
    pub fn start() -> Option<Self> {
        let client_id = client_id()?;
        let (send, progress) = mpsc::channel();
        let session = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (kept, stop) = (session.clone(), cancelled.clone());
        std::thread::Builder::new()
            .name("Microsoft sign-in".into())
            .spawn(move || {
                let result = (|| {
                    let code = device_code(&client_id)?;
                    let _ = send.send(Progress::Code {
                        code: code.user_code.clone(),
                        url: code.verification_uri.clone(),
                    });
                    let ms = wait_for_sign_in(&client_id, &code, &stop)?;
                    let _ = send.send(Progress::Finishing);
                    let (session, secrets) = finish(ms)?;
                    save(&secrets)?;
                    Ok::<_, String>(session)
                })();
                let _ = send.send(match result {
                    Ok(session) => {
                        let name = session.name.clone();
                        if let Ok(mut kept) = kept.lock() {
                            *kept = Some(session);
                        }
                        Progress::Done(name)
                    }
                    Err(e) => Progress::Failed(e),
                });
            })
            .ok()?;
        Some(Self {
            progress,
            session,
            cancelled,
            last: Progress::Requesting,
        })
    }
    /// Where it is now, after what the thread said since.
    pub fn poll(&mut self) -> &Progress {
        while let Ok(progress) = self.progress.try_recv() {
            self.last = progress;
        }
        &self.last
    }
    pub fn take_session(&self) -> Option<AuthSession> {
        self.session.lock().ok()?.take()
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.cancel();
    }
}
