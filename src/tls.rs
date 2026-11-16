//! TLS termination for `--ssl`, including client certificate verification (`--ssl-ca`).
//!
//! rustls does not implement TLS 1.0/1.1 at all, which is at least as strict as ttyd's
//! `SSL_OP_NO_TLSv1 | SSL_OP_NO_TLSv1_1`.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::cli::Config;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub fn server_config(cfg: &Config) -> Result<Arc<ServerConfig>, BoxError> {
    // rustls needs a process-wide crypto provider before any config is built.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert_path = cfg
        .ssl_cert
        .as_deref()
        .ok_or("--ssl-cert is required with --ssl")?;
    let key_path = cfg
        .ssl_key
        .as_deref()
        .ok_or("--ssl-key is required with --ssl")?;
    let certificates = load_certificates(cert_path)?;
    let key = load_private_key(key_path)?;

    let builder = match &cfg.ssl_ca {
        Some(ca) => {
            let mut roots = RootCertStore::empty();
            for certificate in load_certificates(ca)? {
                roots.add(certificate)?;
            }
            let verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build()?;
            ServerConfig::builder().with_client_cert_verifier(verifier)
        }
        None => ServerConfig::builder().with_no_client_auth(),
    };

    let mut config = builder.with_single_cert(certificates, key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn load_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, BoxError> {
    let file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let reader = BufReader::new(file);
    let certificates: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut { reader }).collect::<Result<_, _>>()?;
    if certificates.is_empty() {
        return Err(format!("no certificates found in {}", path.display()).into());
    }
    Ok(certificates)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, BoxError> {
    let file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)?
        .ok_or_else(|| format!("no private key found in {}", path.display()).into())
}

/// Accept loop for a TLS listener. Each connection gets its own rustls session and an
/// HTTP/1.1 connection that supports websocket upgrades.
pub async fn serve(
    listener: TcpListener,
    app: axum::Router,
    config: Arc<ServerConfig>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), BoxError> {
    let acceptor = TlsAcceptor::from(config);
    let mut shutdown = std::pin::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("stopping the TLS listener");
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(pair) => pair,
                    Err(err) => {
                        // A failed accept (fd exhaustion, aborted handshake) must not
                        // take the listener down.
                        tracing::warn!("accept failed: {err}");
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let service = TowerToHyperService::new(app.clone());
                tokio::spawn(async move {
                    let tls = match acceptor.accept(stream).await {
                        Ok(tls) => tls,
                        Err(err) => {
                            tracing::warn!("tls handshake failed from {peer}: {err}");
                            return;
                        }
                    };
                    let io = TokioIo::new(tls);
                    let connection = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .with_upgrades();
                    if let Err(err) = connection.await {
                        tracing::debug!("connection from {peer} ended with: {err}");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use rustls::pki_types::ServerName;

    /// A CA plus server and client certificates signed by it, generated with openssl so
    /// the chain is exactly what an operator would deploy.
    struct Certificates {
        dir: PathBuf,
        ca_pem: PathBuf,
        server_cert: PathBuf,
        server_key: PathBuf,
        client_cert: PathBuf,
        client_key: PathBuf,
    }

    fn openssl_available() -> bool {
        std::process::Command::new("openssl")
            .arg("version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn openssl(script: &str) {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .output()
            .expect("running openssl");
        assert!(
            output.status.success(),
            "openssl failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Generated once per test process: the tests run in parallel and must not race on
    /// the same files.
    fn certificates() -> &'static Certificates {
        static CERTS: std::sync::OnceLock<Certificates> = std::sync::OnceLock::new();
        CERTS.get_or_init(generate_certificates)
    }

    fn generate_certificates() -> Certificates {
        let dir = std::env::temp_dir().join(format!("fttyd-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let ca_key = dir.join("ca.key");
        let ca_pem = dir.join("ca.pem");
        let server_key = dir.join("server.key");
        let server_csr = dir.join("server.csr");
        let server_cert = dir.join("server.pem");
        let client_key = dir.join("client.key");
        let client_csr = dir.join("client.csr");
        let client_cert = dir.join("client.pem");
        let ext = dir.join("server.ext");
        let client_ext = dir.join("client.ext");

        std::fs::write(
            &ext,
            "subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\n",
        )
        .unwrap();
        // `openssl x509 -req` emits a v1 certificate unless extensions are supplied, and
        // rustls refuses v1 certificates outright.
        std::fs::write(
            &client_ext,
            "basicConstraints=CA:FALSE\nextendedKeyUsage=clientAuth\n",
        )
        .unwrap();

        openssl(&format!(
            "openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
             -keyout '{ca_key}' -out '{ca_pem}' -subj '/CN=fttyd-test-ca' \
             -addext 'basicConstraints=critical,CA:TRUE' \
             -addext 'keyUsage=critical,keyCertSign,cRLSign'",
            ca_key = ca_key.display(),
            ca_pem = ca_pem.display(),
        ));
        openssl(&format!(
            "openssl req -newkey rsa:2048 -nodes -keyout '{key}' -out '{csr}' -subj '/CN=localhost' \
             && openssl x509 -req -in '{csr}' -CA '{ca}' -CAkey '{ca_key}' -CAcreateserial -days 2 \
                -extfile '{ext}' -out '{cert}'",
            key = server_key.display(),
            csr = server_csr.display(),
            ca = ca_pem.display(),
            ca_key = ca_key.display(),
            ext = ext.display(),
            cert = server_cert.display(),
        ));
        openssl(&format!(
            "openssl req -newkey rsa:2048 -nodes -keyout '{key}' -out '{csr}' -subj '/CN=fttyd-client' \
             && openssl x509 -req -in '{csr}' -CA '{ca}' -CAkey '{ca_key}' -CAcreateserial -days 2 \
                -extfile '{client_ext}' -out '{cert}'",
            key = client_key.display(),
            csr = client_csr.display(),
            ca = ca_pem.display(),
            ca_key = ca_key.display(),
            client_ext = client_ext.display(),
            cert = client_cert.display(),
        ));

        Certificates {
            dir,
            ca_pem,
            server_cert,
            server_key,
            client_cert,
            client_key,
        }
    }

    fn config_for(certs: &Certificates, client_ca: bool) -> Config {
        Config {
            command: vec!["bash".into()],
            ssl: true,
            ssl_cert: Some(certs.server_cert.clone()),
            ssl_key: Some(certs.server_key.clone()),
            ssl_ca: client_ca.then(|| certs.ca_pem.clone()),
            ..Config::default()
        }
    }

    #[test]
    fn missing_certificate_files_are_reported() {
        if !openssl_available() {
            eprintln!("skipping: openssl is not installed");
            return;
        }
        let certs = certificates();
        let mut config = config_for(certs, false);
        config.ssl_cert = Some(certs.dir.join("does-not-exist.pem"));
        let error = server_config(&config).unwrap_err().to_string();
        assert!(error.contains("does-not-exist.pem"), "{error}");
    }

    #[test]
    fn config_builds_with_and_without_client_verification() {
        if !openssl_available() {
            eprintln!("skipping: openssl is not installed");
            return;
        }
        let certs = certificates();
        assert!(server_config(&config_for(certs, false)).is_ok());
        assert!(server_config(&config_for(certs, true)).is_ok());
    }

    #[tokio::test]
    async fn serves_http_over_tls() {
        if !openssl_available() {
            eprintln!("skipping: openssl is not installed");
            return;
        }
        let certs = certificates();
        let state =
            crate::state::AppState::new(config_for(certs, false), crate::stats::Stats::new());
        let app = crate::server::router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = server_config(&state.cfg).unwrap();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = serve(listener, app, config, async {
                let _ = stop_rx.await;
            })
            .await;
        });

        let client = client_config(certs, true);
        let response = get(client, address, "/api/info").await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let _ = stop_tx.send(());
    }

    #[tokio::test]
    async fn client_certificates_are_required_when_a_ca_is_configured() {
        if !openssl_available() {
            eprintln!("skipping: openssl is not installed");
            return;
        }
        let certs = certificates();
        let state =
            crate::state::AppState::new(config_for(certs, true), crate::stats::Stats::new());
        let app = crate::server::router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = server_config(&state.cfg).unwrap();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = serve(listener, app, config, async {
                let _ = stop_rx.await;
            })
            .await;
        });

        // Without a client certificate the request must fail. TLS 1.3 lets the client
        // handshake "complete" before the server's rejection arrives, so the failure
        // only shows up on the first read.
        let anonymous = client_config(certs, false);
        assert!(get(anonymous, address, "/api/info").await.is_err());

        // With the CA-signed client certificate it succeeds.
        let authenticated = client_config(certs, true);
        let response = get(authenticated, address, "/api/info").await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let _ = stop_tx.send(());
    }

    fn client_config(
        certs: &Certificates,
        with_client_identity: bool,
    ) -> Arc<rustls::ClientConfig> {
        let mut roots = RootCertStore::empty();
        for certificate in load_certificates(&certs.ca_pem).unwrap() {
            roots.add(certificate).unwrap();
        }
        let builder = rustls::ClientConfig::builder().with_root_certificates(roots);
        let config = if with_client_identity {
            let chain = load_certificates(&certs.client_cert).unwrap();
            let key = load_private_key(&certs.client_key).unwrap();
            builder.with_client_auth_cert(chain, key).unwrap()
        } else {
            builder.with_no_client_auth()
        };
        Arc::new(config)
    }

    async fn get(
        config: Arc<rustls::ClientConfig>,
        address: std::net::SocketAddr,
        path: &str,
    ) -> Result<String, String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let client = tokio_rustls::TlsConnector::from(config);
        let stream = tokio::net::TcpStream::connect(address)
            .await
            .map_err(|err| err.to_string())?;
        let name = ServerName::try_from("localhost").unwrap();
        let mut tls = client
            .connect(name, stream)
            .await
            .map_err(|err| err.to_string())?;
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        tls.write_all(request.as_bytes())
            .await
            .map_err(|err| err.to_string())?;
        tls.flush().await.map_err(|err| err.to_string())?;
        let mut response = Vec::new();
        tls.read_to_end(&mut response)
            .await
            .map_err(|err| err.to_string())?;
        if response.is_empty() {
            return Err("server closed the connection without a response".to_string());
        }
        Ok(String::from_utf8_lossy(&response).into_owned())
    }
}
