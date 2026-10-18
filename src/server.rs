//! Embedded WebDAV server.
//!
//! Replaces the `hacdias/webdav` Docker container from upstream's `server/`
//! directory: the desktop app serves its own note directory, so the Android
//! app syncs straight to the machine you're editing on.

use anyhow::Result;
use base64::Engine;
use dav_server::{fakels::FakeLs, localfs::LocalFs, DavHandler};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::oneshot;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub root: PathBuf,
}

pub struct ServerHandle {
    shutdown: Option<oneshot::Sender<()>>,
    runtime: tokio::runtime::Runtime,
    running: Arc<AtomicBool>,
}

impl ServerHandle {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.running.store(false, Ordering::SeqCst);
        // Give in-flight requests a moment to drain before tearing the runtime down.
        self.runtime
            .shutdown_timeout(std::time::Duration::from_secs(2));
    }
}

/// Starts the server on its own Tokio runtime thread and returns once the
/// listener is bound, so a port conflict surfaces as an error in the UI rather
/// than as a silent no-op.
pub fn start(config: ServerConfig) -> Result<ServerHandle> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    let addr: SocketAddr = format!("{}:{}", config.bind, config.port).parse()?;
    let listener = runtime.block_on(async { tokio::net::TcpListener::bind(addr).await })?;

    let dav = DavHandler::builder()
        .filesystem(LocalFs::new(config.root.clone(), false, false, false))
        .locksystem(FakeLs::new())
        .build_handler();

    let (tx, rx) = oneshot::channel::<()>();
    let running = Arc::new(AtomicBool::new(true));
    let running_task = running.clone();
    let expected = Arc::new((config.username.clone(), config.password.clone()));

    runtime.spawn(async move {
        let mut shutdown = rx;
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let dav = dav.clone();
                    let expected = expected.clone();
                    tokio::spawn(async move {
                        let io = hyper_util::rt::TokioIo::new(stream);
                        let service = hyper::service::service_fn(move |req| {
                            let dav = dav.clone();
                            let expected = expected.clone();
                            async move { handle(dav, expected, req).await }
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, service)
                            .await;
                    });
                }
            }
        }
        running_task.store(false, Ordering::SeqCst);
    });

    Ok(ServerHandle {
        shutdown: Some(tx),
        runtime,
        running,
    })
}

type Body = dav_server::body::Body;

async fn handle(
    dav: DavHandler,
    expected: Arc<(String, String)>,
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Body>, std::convert::Infallible> {
    if !authorized(&req, &expected) {
        let mut res = hyper::Response::new(Body::from("Unauthorized".to_string()));
        *res.status_mut() = hyper::StatusCode::UNAUTHORIZED;
        res.headers_mut().insert(
            hyper::header::WWW_AUTHENTICATE,
            hyper::header::HeaderValue::from_static("Basic realm=\"Notes\""),
        );
        return Ok(res);
    }
    Ok(dav.handle(req).await)
}

fn authorized<B>(req: &hyper::Request<B>, expected: &(String, String)) -> bool {
    let Some(header) = req
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(encoded) = header.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
        return false;
    };
    let Ok(pair) = String::from_utf8(decoded) else {
        return false;
    };
    let Some((user, pass)) = pair.split_once(':') else {
        return false;
    };
    // Compare both halves unconditionally so the check doesn't leak which one
    // was wrong through its timing.
    let user_ok = constant_time_eq(user.as_bytes(), expected.0.as_bytes());
    let pass_ok = constant_time_eq(pass.as_bytes(), expected.1.as_bytes());
    user_ok & pass_ok
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Addresses to show in the UI so the phone can be pointed at this machine.
pub fn local_urls(port: u16) -> Vec<String> {
    let mut urls = vec![format!("http://localhost:{port}/")];
    if let Ok(output) = std::process::Command::new("hostname").arg("-I").output() {
        for ip in String::from_utf8_lossy(&output.stdout).split_whitespace() {
            if !ip.contains(':') {
                urls.push(format!("http://{ip}:{port}/"));
            }
        }
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(header: Option<&str>) -> hyper::Request<()> {
        let mut b = hyper::Request::builder();
        if let Some(h) = header {
            b = b.header(hyper::header::AUTHORIZATION, h);
        }
        b.body(()).unwrap()
    }

    #[test]
    fn accepts_matching_basic_credentials() {
        let expected = ("noteuser".to_string(), "s3cret".to_string());
        let encoded = base64::engine::general_purpose::STANDARD.encode("noteuser:s3cret");
        assert!(authorized(
            &req(Some(&format!("Basic {encoded}"))),
            &expected
        ));
    }

    #[test]
    fn rejects_wrong_password_and_missing_header() {
        let expected = ("noteuser".to_string(), "s3cret".to_string());
        let encoded = base64::engine::general_purpose::STANDARD.encode("noteuser:wrong");
        assert!(!authorized(
            &req(Some(&format!("Basic {encoded}"))),
            &expected
        ));
        assert!(!authorized(&req(None), &expected));
    }

    #[test]
    fn rejects_password_containing_a_colon_only_when_wrong() {
        let expected = ("u".to_string(), "a:b".to_string());
        let encoded = base64::engine::general_purpose::STANDARD.encode("u:a:b");
        assert!(authorized(
            &req(Some(&format!("Basic {encoded}"))),
            &expected
        ));
    }
}
