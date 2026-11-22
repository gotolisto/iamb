//! The transport over HTTPS (§6.1): `POST <project>/rest/v1/rpc/<function>` with the `api` schema
//! selected, the project's public key and the account's access token; and the two calls of the
//! authentication service the agent makes (renewing the access token, signing out).
//!
//! The HTTP client is `ureq` (MIT or Apache-2.0): blocking, small, no async runtime, and TLS by
//! `rustls` (Apache-2.0, ISC or MIT) with the `ring` provider rather than OpenSSL. The trusted
//! roots are the operating system's (`rustls-native-certs`, Apache-2.0, ISC or MIT), which also
//! honours `SSL_CERT_FILE`, so a machine behind an inspecting proxy works as it does for the system.
//! A proxy named in the environment (`HTTPS_PROXY`) is used. A call has 40 seconds (§6.1).
//!
//! Neither the tokens nor any argument or answer is logged.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;

use crate::api::{CallError, Transport};
use crate::auth::Tokens;
use crate::schedule::CALL_TIMEOUT_MS;

/// The project: its URL and its public (publishable) key.
#[derive(Clone)]
pub struct Project {
    /// `https://<ref>.supabase.co`.
    pub url: String,
    /// The public key the REST interface asks for alongside the access token.
    pub public_key: String,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("url", &self.url)
            .field("public_key", &"[redacted]")
            .finish()
    }
}

/// The TLS roots of the system and the `ring` provider, for `ureq` and for the live channel.
///
/// # Errors
///
/// No root certificate could be loaded.
pub fn root_certs() -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let found = rustls_native_certs::load_native_certs();
    if found.certs.is_empty() {
        return Err("no trusted root certificates found on this system".into());
    }
    Ok(found.certs)
}

/// A `rustls` client configuration with the system's roots.
///
/// # Errors
///
/// No roots, or the provider refused the configuration.
pub fn rustls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    for c in root_certs()? {
        // A certificate the store cannot parse is skipped, as the system would.
        let _ = roots.add(c);
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// A `ureq` agent with the system's roots, the 40-second time-out, and HTTP statuses as answers.
///
/// # Errors
///
/// No roots.
pub fn http_agent() -> Result<ureq::Agent, String> {
    let certs: Vec<ureq::tls::Certificate<'static>> = root_certs()?
        .into_iter()
        .map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned())
        .collect();
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::Rustls)
        .root_certs(ureq::tls::RootCerts::new_with_certs(&certs))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(CALL_TIMEOUT_MS)))
        .http_status_as_error(false)
        .tls_config(tls)
        .build();
    Ok(config.into())
}

fn call_error(e: &ureq::Error) -> CallError {
    match e {
        ureq::Error::Timeout(_) => CallError::Timeout,
        ureq::Error::StatusCode(s) => CallError::Status(*s),
        other => CallError::Network(other.to_string()),
    }
}

/// The REST interface of the project, with the session's access token.
pub struct HttpTransport {
    agent: ureq::Agent,
    project: Project,
    tokens: Arc<Mutex<Option<Tokens>>>,
}

impl HttpTransport {
    /// A transport for `project` that reads the access token from `tokens` at every call.
    #[must_use]
    pub const fn new(
        agent: ureq::Agent,
        project: Project,
        tokens: Arc<Mutex<Option<Tokens>>>,
    ) -> Self {
        Self {
            agent,
            project,
            tokens,
        }
    }
}

impl Transport for HttpTransport {
    fn call(&mut self, function: &str, args: Value) -> Result<Value, CallError> {
        let access = self
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|t| t.access_token.clone())
            .ok_or(CallError::Unauthorized)?;
        let url = format!(
            "{}/rest/v1/rpc/{function}",
            self.project.url.trim_end_matches('/')
        );
        let body = serde_json::to_string(&args).map_err(|e| CallError::Malformed(e.to_string()))?;
        let mut resp = self
            .agent
            .post(&url)
            .header("apikey", &self.project.public_key)
            .header("Authorization", &format!("Bearer {access}"))
            .header("Content-Type", "application/json")
            .header("Content-Profile", "api")
            .header("Accept", "application/json")
            .send(body)
            .map_err(|e| call_error(&e))?;
        let status = resp.status().as_u16();
        if status == 401 {
            return Err(CallError::Unauthorized);
        }
        if status != 200 {
            return Err(CallError::Status(status));
        }
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| call_error(&e))?;
        serde_json::from_str(&text).map_err(|e| CallError::Malformed(e.to_string()))
    }
}

/// What the authentication service said to a refresh.
#[derive(Debug)]
pub enum Refresh {
    /// New tokens.
    Renewed(Tokens),
    /// The refresh token was refused: the session ended without a word from Iamb's server (§6.1).
    Refused,
    /// No answer.
    Failed(CallError),
}

/// The authentication service of the project.
pub struct AuthClient {
    agent: ureq::Agent,
    project: Project,
}

impl AuthClient {
    /// A client for `project`.
    #[must_use]
    pub const fn new(agent: ureq::Agent, project: Project) -> Self {
        Self { agent, project }
    }

    /// Renews the access token with the refresh token (`grant_type=refresh_token`).
    #[must_use]
    pub fn refresh(&self, refresh_token: &str, now_secs: i64) -> Refresh {
        let url = format!(
            "{}/auth/v1/token?grant_type=refresh_token",
            self.project.url.trim_end_matches('/')
        );
        let body = serde_json::json!({ "refresh_token": refresh_token }).to_string();
        let resp = self
            .agent
            .post(&url)
            .header("apikey", &self.project.public_key)
            .header("Content-Type", "application/json")
            .send(body);
        let mut resp = match resp {
            Ok(r) => r,
            Err(e) => return Refresh::Failed(call_error(&e)),
        };
        let status = resp.status().as_u16();
        if matches!(status, 400 | 401 | 403) {
            return Refresh::Refused;
        }
        if status != 200 {
            return Refresh::Failed(CallError::Status(status));
        }
        let Ok(text) = resp.body_mut().read_to_string() else {
            return Refresh::Failed(CallError::Malformed("unreadable answer".into()));
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            return Refresh::Failed(CallError::Malformed("not JSON".into()));
        };
        let (Some(access), Some(refresh)) = (
            v.get("access_token").and_then(Value::as_str),
            v.get("refresh_token").and_then(Value::as_str),
        ) else {
            return Refresh::Failed(CallError::Malformed("no tokens".into()));
        };
        let expires_at = v
            .get("expires_at")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                now_secs + v.get("expires_in").and_then(Value::as_i64).unwrap_or(3_600)
            });
        Refresh::Renewed(Tokens {
            access_token: access.to_owned(),
            refresh_token: refresh.to_owned(),
            expires_at,
        })
    }

    /// Ends the session at the authentication service: `local` (this one) or `global` (all of the
    /// user's, after `sign_out_everywhere`, C25).
    ///
    /// # Errors
    ///
    /// [`CallError`].
    pub fn sign_out(&self, access_token: &str, scope: &str) -> Result<(), CallError> {
        let url = format!(
            "{}/auth/v1/logout?scope={scope}",
            self.project.url.trim_end_matches('/')
        );
        let resp = self
            .agent
            .post(&url)
            .header("apikey", &self.project.public_key)
            .header("Authorization", &format!("Bearer {access_token}"))
            .send_empty()
            .map_err(|e| call_error(&e))?;
        match resp.status().as_u16() {
            200..=299 | 401 | 403 | 404 => Ok(()),
            s => Err(CallError::Status(s)),
        }
    }
}
