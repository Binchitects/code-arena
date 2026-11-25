//! The HTTP client every part of Code Arena uses: the system's CAs and the company's (`--ca`), TCP keep-alive so a
//! server gone without a word is found out, Code Arena's User-Agent, and errors in words a person can act on.

use std::path::Path;
use std::time::Duration;

/// Why a request did not get an answer, said plainly.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("the certificate of {host} is not trusted: give the company's CA with --ca (or ARENA_CA_CERT)")]
    Certificate { host: String },
    #[error("the name in {url} does not resolve: check the address, the VPN and DNS")]
    Resolve { url: String },
    #[error("{url} did not answer in time")]
    Timeout { url: String },
    #[error("{url} could not be reached: {message}")]
    Connect { url: String, message: String },
    #[error("the CA file {path} could not be read: {message}")]
    CaFile { path: String, message: String },
    #[error("{0}")]
    Other(String),
}

/// A client for an Arena and its gateway: the extra CA trusted besides the system's.
pub fn client(ca: Option<&Path>) -> Result<reqwest::Client, NetError> {
    builder(ca).map(|b| b.cookie_store(false)).and_then(|b| b.build().map_err(|e| NetError::Other(e.to_string())))
}

/// One with its own cookies, for a password sign-in that lives only as long as it.
pub fn client_with_cookies(ca: Option<&Path>) -> Result<reqwest::Client, NetError> {
    builder(ca).map(|b| b.cookie_store(true)).and_then(|b| b.build().map_err(|e| NetError::Other(e.to_string())))
}

fn builder(ca: Option<&Path>) -> Result<reqwest::ClientBuilder, NetError> {
    let mut b = reqwest::Client::builder()
        .user_agent(ca_core::user_agent())
        .connect_timeout(Duration::from_secs(15))
        // A probe after 30 s of quiet: a server that went away (a laptop asleep, a VPN changed) is found out in about
        // a minute and a half rather than never.
        .tcp_keepalive(Duration::from_secs(30))
        .use_rustls_tls();
    if let Some(path) = ca {
        let pem = std::fs::read(path)
            .map_err(|e| NetError::CaFile { path: path.display().to_string(), message: e.to_string() })?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| NetError::CaFile { path: path.display().to_string(), message: e.to_string() })?;
        if certs.is_empty() {
            return Err(NetError::CaFile {
                path: path.display().to_string(),
                message: "it holds no certificate".into(),
            });
        }
        for c in certs {
            b = b.add_root_certificate(c);
        }
    }
    Ok(b)
}

/// A transport error in words: a certificate, a name, a timeout, a refused connection.
pub fn explain(e: &reqwest::Error) -> NetError {
    let url = e.url().map(|u| u.as_str().to_string()).unwrap_or_default();
    let host = e.url().and_then(|u| u.host_str()).unwrap_or_default().to_string();
    let chain = chain_text(e);
    if e.is_timeout() {
        return NetError::Timeout { url };
    }
    let lower = chain.to_ascii_lowercase();
    if lower.contains("certificate") || lower.contains("unknownissuer") || lower.contains("invalid peer") {
        return NetError::Certificate { host };
    }
    if lower.contains("dns error") || lower.contains("failed to lookup") || lower.contains("name or service not known")
    {
        return NetError::Resolve { url };
    }
    NetError::Connect { url, message: chain }
}

fn chain_text(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let t = s.to_string();
        if !parts.iter().any(|p| p.contains(&t)) {
            parts.push(t);
        }
        cur = s.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sends_its_user_agent() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::header("user-agent", ca_core::user_agent().as_str()))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let res = client(None).expect("client").get(server.uri()).send().await.expect("answers");
        assert_eq!(res.status(), 200);
    }

    #[test]
    fn a_ca_file_that_is_not_there_is_said() {
        let err = client(Some(Path::new("/no/such/ca.crt"))).expect_err("fails");
        assert!(matches!(err, NetError::CaFile { .. }));
    }

    #[tokio::test]
    async fn a_refused_connection_is_said() {
        // A port nothing listens on: bind one, then let it go.
        let port = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
        let err =
            client(None).expect("client").get(format!("http://127.0.0.1:{port}/")).send().await.expect_err("refused");
        assert!(matches!(explain(&err), NetError::Connect { .. }), "{:?}", explain(&err));
    }
}
