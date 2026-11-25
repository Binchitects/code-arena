//! Signing in with a user name and password, as Arena's sign-in page does, so its lockout, its directory and its
//! second factor hold. A key of the person's own is then made for this machine and the web session signed out:
//! the key is what Code Arena keeps.

use std::path::Path;
use std::time::Duration;

use ca_core::Secret;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::client::ArenaError;

pub struct PasswordSignIn {
    base: String,
    http: reqwest::Client,
}

/// What the password step said.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    SignedIn,
    /// The account has a second factor: [`PasswordSignIn::code`] next.
    SecondFactor,
}

impl PasswordSignIn {
    pub fn new(base: &str, ca: Option<&Path>) -> Result<Self, ArenaError> {
        Ok(Self { base: base.trim_end_matches('/').to_string(), http: ca_net::client_with_cookies(ca)? })
    }

    /// The name of the key made for this machine.
    pub fn key_name(machine: &str) -> String {
        format!("Code Arena on {machine}")
    }

    pub async fn password(&self, user: &str, password: &Secret) -> Result<Step, ArenaError> {
        let said = self
            .send(
                Method::POST,
                "/api/auth/login",
                Some(json!({"userName": user, "password": password.expose(), "remember": false})),
            )
            .await?;
        Ok(if said.get("status").and_then(Value::as_str) == Some("2fa") { Step::SecondFactor } else { Step::SignedIn })
    }

    /// The code from the authenticator app, or a recovery code (with its hyphen).
    pub async fn code(&self, code: &Secret) -> Result<(), ArenaError> {
        let c = code.expose().trim();
        self.send(
            Method::POST,
            "/api/auth/login/2fa",
            Some(json!({"code": c, "recovery": c.contains('-'), "remember": false})),
        )
        .await
        .map(|_| ())
    }

    /// Signed in: a key made for this machine (one of that name before is revoked first), then the web session
    /// signed out, whatever happened.
    pub async fn make_key(&self, machine: &str) -> Result<Secret, ArenaError> {
        let made = self.make_key_inner(machine).await;
        let _ = self.send(Method::POST, "/api/auth/logout", Some(json!({}))).await;
        made
    }

    async fn make_key_inner(&self, machine: &str) -> Result<Secret, ArenaError> {
        let name = Self::key_name(machine);
        let mine = self.send(Method::GET, "/api/account/keys/own", None).await?;
        for old in mine.get("keys").and_then(Value::as_array).into_iter().flatten() {
            let own = old.get("kind").and_then(Value::as_str) == Some("own");
            if own
                && old.get("name").and_then(Value::as_str) == Some(name.as_str())
                && let Some(id) = old.get("id").and_then(Value::as_str)
            {
                self.send(Method::DELETE, &format!("/api/account/keys/own/{id}"), None).await?;
            }
        }
        let made = self.send(Method::POST, "/api/account/keys/own", Some(json!({"name": name}))).await?;
        match made.get("apiKey").and_then(Value::as_str) {
            Some(k) if !k.is_empty() => Ok(Secret::new(k)),
            _ => Err(ArenaError::Status { status: 200, code: None, message: "Arena made no key.".into() }),
        }
    }

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, ArenaError> {
        let mut r = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("X-Requested-With", "code-arena")
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(30));
        if let Some(b) = &body {
            r = r.json(b);
        }
        let res = r.send().await.map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        let status = res.status();
        let text = res.text().await.map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        let said: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(said);
        }
        let error = said.get("error").and_then(Value::as_str).map(str::to_string);
        let code = said.get("status").and_then(Value::as_str).map(str::to_string);
        let message = match (status, error) {
            (StatusCode::NOT_FOUND, _) if path == "/api/auth/login" => {
                format!("{} is not an Arena that signs people in with a password: check the address", self.base)
            }
            (StatusCode::FORBIDDEN, None) => {
                "Arena refused: sign in on its web page once (to accept its notice or set up two-factor sign-in), then here".into()
            }
            (_, Some(e)) => e,
            (s, None) => format!("Arena answered {} {}", s.as_u16(), s.canonical_reason().unwrap_or("")),
        };
        Err(ArenaError::Status { status: status.as_u16(), code, message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn password_then_code_then_a_key_for_this_machine() {
        let server = MockServer::start().await;
        Mock::given(path("/api/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "2fa"})))
            .mount(&server)
            .await;
        Mock::given(path("/api/auth/login/2fa"))
            .and(body_partial_json(json!({"recovery": false})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/account/keys/own"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"keys": [{"id": "k1", "kind": "own", "name": "Code Arena on box"}, {"id": "k2", "kind": "own", "name": "other"}]}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/account/keys/own/k1"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/account/keys/own"))
            .and(body_partial_json(json!({"name": "Code Arena on box"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"apiKey": "sk-made"})))
            .mount(&server)
            .await;
        Mock::given(path("/api/auth/logout")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&server).await;

        let s = PasswordSignIn::new(&server.uri(), None).expect("client");
        assert_eq!(s.password("sara", &Secret::new("pw")).await.expect("password"), Step::SecondFactor);
        s.code(&Secret::new("123456")).await.expect("code");
        assert_eq!(s.make_key("box").await.expect("key").expose(), "sk-made");
    }

    #[tokio::test]
    async fn a_wrong_password_in_arenas_words() {
        let server = MockServer::start().await;
        Mock::given(path("/api/auth/login"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(json!({"status": "invalid", "error": "Wrong user name or password."})),
            )
            .mount(&server)
            .await;
        let s = PasswordSignIn::new(&server.uri(), None).expect("client");
        let err = s.password("sara", &Secret::new("nope")).await.expect_err("refused");
        assert_eq!(err.to_string(), "Arena answered 400: Wrong user name or password.");
    }
}
