use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::io;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

const OTP_LIFETIME: Duration = Duration::from_secs(5 * 60);
const SESSION_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
const CHPASSWD: &str = "/usr/sbin/chpasswd";

static CRYPT_LOCK: StdMutex<()> = StdMutex::new(());
static CRYPT_FUNCTION: OnceLock<Result<usize, String>> = OnceLock::new();
type CryptFunction = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char;

#[derive(Debug, Clone)]
struct Otp {
    code: String,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
struct Session {
    expires_at: Instant,
    password_hash: String,
}

#[derive(Clone)]
pub struct AuthState {
    otp: Arc<Mutex<Option<Otp>>>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    #[cfg(test)]
    test_password: Option<Arc<StdMutex<String>>>,
}

impl AuthState {
    pub fn open() -> Self {
        Self {
            otp: Arc::new(Mutex::new(None)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            test_password: None,
        }
    }

    #[cfg(test)]
    fn with_password(password: &str) -> Self {
        Self {
            otp: Arc::new(Mutex::new(None)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            test_password: Some(Arc::new(StdMutex::new(password.to_owned()))),
        }
    }

    pub async fn has_password(&self) -> bool {
        self.password_hash()
            .await
            .is_some_and(|hash| !hash.is_empty() && hash != "!" && hash != "*")
    }

    pub async fn issue_otp(&self) -> String {
        let code = format!("{:06}", rand::random_range(0..1_000_000_u32));
        *self.otp.lock().await = Some(Otp {
            code: code.clone(),
            expires_at: Instant::now() + OTP_LIFETIME,
        });
        code
    }

    pub async fn clear_expired_otp(&self) {
        let mut otp = self.otp.lock().await;
        if otp
            .as_ref()
            .is_some_and(|otp| otp.expires_at <= Instant::now())
        {
            *otp = None;
        }
    }

    pub async fn authenticate_password(&self, password: &str) -> Option<String> {
        let password_hash = self.verify_password(password).await.ok().flatten()?;
        Some(self.create_session(password_hash).await)
    }

    pub async fn recover_password(&self, otp: &str, password: &str) -> io::Result<Option<String>> {
        validate_password(password)?;
        let mut active = self.otp.lock().await;
        let matches = active
            .as_ref()
            .is_some_and(|active| active.expires_at > Instant::now() && active.code == otp);
        if !matches {
            return Ok(None);
        }
        *active = None;
        drop(active);
        self.set_password(password).await?;
        self.sessions.lock().await.clear();
        let hash = self
            .password_hash()
            .await
            .ok_or_else(|| io::Error::other("root password hash is unavailable"))?;
        Ok(Some(self.create_session(hash).await))
    }

    pub async fn change_password(&self, current: &str, password: &str) -> io::Result<bool> {
        validate_password(password)?;
        if self.verify_password(current).await?.is_none() {
            return Ok(false);
        }
        self.set_password(password).await?;
        self.sessions.lock().await.clear();
        Ok(true)
    }

    pub async fn validate_session(&self, token: &str) -> bool {
        let current_hash = match self.password_hash().await {
            Some(hash) => hash,
            None => return false,
        };
        let mut sessions = self.sessions.lock().await;
        let now = Instant::now();
        sessions
            .retain(|_, session| session.expires_at > now && session.password_hash == current_hash);
        sessions.contains_key(token)
    }

    pub async fn remove_session(&self, token: &str) {
        self.sessions.lock().await.remove(token);
    }

    async fn create_session(&self, password_hash: String) -> String {
        let token = hex::encode(rand::random::<[u8; 32]>());
        self.sessions.lock().await.insert(
            token.clone(),
            Session {
                expires_at: Instant::now() + SESSION_LIFETIME,
                password_hash,
            },
        );
        token
    }

    async fn password_hash(&self) -> Option<String> {
        #[cfg(test)]
        if let Some(password) = &self.test_password {
            return password.lock().ok().map(|password| password.clone());
        }
        tokio::task::spawn_blocking(read_root_password_hash)
            .await
            .ok()
            .and_then(Result::ok)
    }

    async fn verify_password(&self, password: &str) -> io::Result<Option<String>> {
        #[cfg(test)]
        if let Some(expected) = &self.test_password {
            return Ok(expected
                .lock()
                .ok()
                .filter(|expected| expected.as_str() == password)
                .map(|expected| expected.clone()));
        }
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || verify_root_password(&password))
            .await
            .map_err(io::Error::other)?
    }

    async fn set_password(&self, password: &str) -> io::Result<()> {
        #[cfg(test)]
        if let Some(current) = &self.test_password {
            *current
                .lock()
                .map_err(|_| io::Error::other("test password lock is poisoned"))? =
                password.to_owned();
            return Ok(());
        }
        let mut child = tokio::process::Command::new(CHPASSWD)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("chpasswd stdin is unavailable"))?
            .write_all(format!("root:{password}\n").as_bytes())
            .await?;
        let output = child.wait_with_output().await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }
}

fn read_root_password_hash() -> io::Result<String> {
    let shadow = std::fs::read_to_string("/etc/shadow")?;
    shadow
        .lines()
        .find_map(|line| {
            let mut fields = line.split(':');
            (fields.next() == Some("root")).then(|| fields.next().unwrap_or_default().to_owned())
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "root shadow entry is missing"))
}

fn verify_root_password(password: &str) -> io::Result<Option<String>> {
    let hash = read_root_password_hash()?;
    if hash.is_empty() || hash == "!" || hash == "*" {
        return Ok(None);
    }
    let password = CString::new(password)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "password contains NUL"))?;
    let salt = CString::new(hash.as_str()).expect("shadow hashes do not contain NUL");
    let _guard = CRYPT_LOCK
        .lock()
        .map_err(|_| io::Error::other("crypt lock is poisoned"))?;
    let function = crypt_function()?;
    let result = unsafe { function(password.as_ptr(), salt.as_ptr()) };
    if result.is_null() {
        return Err(io::Error::last_os_error());
    }
    let verified = unsafe { CStr::from_ptr(result) }.to_bytes() == hash.as_bytes();
    Ok(verified.then_some(hash))
}

fn crypt_function() -> io::Result<CryptFunction> {
    let address = CRYPT_FUNCTION.get_or_init(|| unsafe {
        let library = libc::dlopen(c"libcrypt.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if library.is_null() {
            return Err(dl_error());
        }
        let symbol = libc::dlsym(library, c"crypt".as_ptr());
        if symbol.is_null() {
            return Err(dl_error());
        }
        Ok(symbol as usize)
    });
    match address {
        Ok(address) => Ok(unsafe { std::mem::transmute::<usize, CryptFunction>(*address) }),
        Err(error) => Err(io::Error::other(error.clone())),
    }
}

unsafe fn dl_error() -> String {
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        "dynamic library operation failed".to_owned()
    } else {
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    }
}

fn validate_password(password: &str) -> io::Result<()> {
    if !(8..=128).contains(&password.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "password must contain 8 to 128 characters",
        ));
    }
    if password.contains(['\0', '\n', '\r', ':']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "password contains an unsupported character",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn otp_changes_password_once_and_invalidates_sessions() {
        let auth = AuthState::with_password("old-password");
        let old_token = auth.authenticate_password("old-password").await.unwrap();
        let otp = auth.issue_otp().await;
        let token = auth
            .recover_password(&otp, "new-password")
            .await
            .unwrap()
            .unwrap();
        assert!(!auth.validate_session(&old_token).await);
        assert!(auth.validate_session(&token).await);
        assert!(
            auth.recover_password(&otp, "other-password")
                .await
                .unwrap()
                .is_none()
        );
        assert!(auth.authenticate_password("new-password").await.is_some());
    }

    #[test]
    fn rejects_chpasswd_delimiters() {
        assert!(validate_password("bad:password").is_err());
        assert!(validate_password("bad\npassword").is_err());
    }
}
