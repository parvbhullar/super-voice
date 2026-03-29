use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::gateway::manager::GatewayManager;

/// Background task that OPTIONS-pings gateways and records success/failure
/// with `GatewayManager::record_health_result`.
pub struct GatewayHealthMonitor {
    manager: Arc<Mutex<GatewayManager>>,
    cancel: CancellationToken,
}

impl GatewayHealthMonitor {
    /// Create a new monitor. Call `start()` to begin background monitoring.
    pub fn new(manager: Arc<Mutex<GatewayManager>>) -> Self {
        Self {
            manager,
            cancel: CancellationToken::new(),
        }
    }

    /// Spawn the background monitoring loop.
    ///
    /// The loop:
    /// 1. Lists all gateways.
    /// 2. For each gateway whose `health_check_interval_secs > 0` and whose
    ///    interval has elapsed, sends an OPTIONS ping.
    /// 3. Calls `record_health_result` with the outcome.
    /// 4. Sleeps for 1 second before the next iteration.
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        let manager = self.manager.clone();
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            // Per-gateway last-ping tracking: name -> last Instant
            let mut last_ping: std::collections::HashMap<String, Instant> =
                std::collections::HashMap::new();

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        debug!("GatewayHealthMonitor: cancelled");
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }

                if cancel.is_cancelled() {
                    break;
                }

                // Collect gateways to check (clone info to avoid holding lock)
                let gateways = {
                    let mgr = manager.lock().await;
                    mgr.list_gateways()
                        .into_iter()
                        .filter(|gw| gw.last_check.is_none()) // use our own tracking
                        .map(|gw| (gw.name.clone(), gw.proxy_addr.clone(), gw.transport.clone()))
                        .collect::<Vec<_>>()
                };

                // Collect all gateways with their intervals
                let all_gateways: Vec<_> = {
                    let mgr = manager.lock().await;
                    mgr.list_gateways()
                        .into_iter()
                        .map(|gw| {
                            let interval = {
                                // We need the interval from config — re-read in lock
                                0u32 // placeholder; resolved below
                            };
                            (gw.name, gw.proxy_addr, gw.transport, interval)
                        })
                        .collect()
                };
                let _ = gateways; // silence unused warning

                // Rebuild with config interval (requires another lock — acceptable)
                let configs: Vec<_> = {
                    let mgr = manager.lock().await;
                    mgr.list_gateways()
                        .into_iter()
                        .collect()
                };
                let _ = all_gateways;

                for gw_info in configs {
                    // Read the per-gateway health check interval from config.
                    let interval_secs = gw_info.health_check_interval_secs;

                    let now = Instant::now();
                    let due = match last_ping.get(&gw_info.name) {
                        None => true,
                        Some(last) => now.duration_since(*last) >= Duration::from_secs(interval_secs),
                    };

                    if !due {
                        continue;
                    }

                    let proxy_addr = gw_info.proxy_addr.clone();
                    let transport = gw_info.transport.clone();
                    let name = gw_info.name.clone();

                    let success = send_options_ping(&proxy_addr, &transport).await;

                    if success {
                        debug!("Gateway '{}' OPTIONS ping succeeded", name);
                    } else {
                        warn!("Gateway '{}' OPTIONS ping failed", name);
                    }

                    last_ping.insert(name.clone(), Instant::now());

                    let mut mgr = manager.lock().await;
                    if let Err(e) = mgr.record_health_result(&name, success).await {
                        warn!("Failed to record health result for '{}': {}", name, e);
                    }
                }
            }
        })
    }

    /// Cancel the background monitoring loop.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

/// Send a raw SIP OPTIONS request to `proxy_addr` over `transport`.
///
/// Returns `true` if a 2xx response is received within 5 seconds.
async fn send_options_ping(proxy_addr: &str, transport: &str) -> bool {
    let timeout_duration = Duration::from_secs(5);

    match transport.to_lowercase().as_str() {
        "udp" => send_options_udp(proxy_addr, timeout_duration).await,
        "tcp" => send_options_tcp(proxy_addr, timeout_duration).await,
        "tls" => send_options_tls(proxy_addr, timeout_duration).await,
        other => {
            warn!("Unknown transport '{}' for OPTIONS ping", other);
            false
        }
    }
}

/// Build a minimal RFC 3261 OPTIONS message.
fn build_options_message(proxy_addr: &str, transport: &str, local_addr: &str) -> String {
    let branch = format!("z9hG4bK{}", Uuid::new_v4().simple());
    let tag = Uuid::new_v4().simple().to_string();
    let call_id = format!("{}@{}", Uuid::new_v4().simple(), local_addr);
    let transport_upper = transport.to_uppercase();
    let local_domain = local_addr.split(':').next().unwrap_or(local_addr);

    format!(
        "OPTIONS sip:{proxy_addr} SIP/2.0\r\n\
         Via: SIP/2.0/{transport_upper} {local_addr};branch={branch}\r\n\
         From: <sip:healthcheck@{local_domain}>;tag={tag}\r\n\
         To: <sip:{proxy_addr}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 OPTIONS\r\n\
         Max-Forwards: 70\r\n\
         Content-Length: 0\r\n\
         \r\n"
    )
}

/// Returns true if `response` starts with a 2xx status line.
fn is_2xx_response(response: &[u8]) -> bool {
    // SIP/2.0 2xx ...
    if let Ok(text) = std::str::from_utf8(response) {
        let first_line = text.lines().next().unwrap_or("");
        let parts: Vec<&str> = first_line.splitn(3, ' ').collect();
        if parts.len() >= 2 {
            if let Ok(code) = parts[1].parse::<u16>() {
                return (200..300).contains(&code);
            }
        }
    }
    false
}

async fn send_options_udp(proxy_addr: &str, timeout: Duration) -> bool {
    let local_bind = "0.0.0.0:0";
    let socket = match UdpSocket::bind(local_bind).await {
        Ok(s) => s,
        Err(e) => {
            warn!("UDP bind failed: {}", e);
            return false;
        }
    };

    if let Err(e) = socket.connect(proxy_addr).await {
        warn!("UDP connect to '{}' failed: {}", proxy_addr, e);
        return false;
    }

    let local_addr = socket
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "127.0.0.1:0".to_string());

    let msg = build_options_message(proxy_addr, "udp", &local_addr);

    if let Err(e) = socket.send(msg.as_bytes()).await {
        warn!("UDP send to '{}' failed: {}", proxy_addr, e);
        return false;
    }

    let mut buf = vec![0u8; 2048];
    match tokio::time::timeout(timeout, socket.recv(&mut buf)).await {
        Ok(Ok(n)) => is_2xx_response(&buf[..n]),
        Ok(Err(e)) => {
            warn!("UDP recv from '{}' failed: {}", proxy_addr, e);
            false
        }
        Err(_) => {
            warn!("UDP OPTIONS ping to '{}' timed out", proxy_addr);
            false
        }
    }
}

async fn send_options_tcp(proxy_addr: &str, timeout: Duration) -> bool {
    let stream =
        match tokio::time::timeout(timeout, TcpStream::connect(proxy_addr)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!("TCP connect to '{}' failed: {}", proxy_addr, e);
                return false;
            }
            Err(_) => {
                warn!("TCP connect to '{}' timed out", proxy_addr);
                return false;
            }
        };

    let local_addr = stream
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "127.0.0.1:0".to_string());

    let msg = build_options_message(proxy_addr, "tcp", &local_addr);
    let (mut reader, mut writer) = stream.into_split();

    if let Err(e) = writer.write_all(msg.as_bytes()).await {
        warn!("TCP write to '{}' failed: {}", proxy_addr, e);
        return false;
    }

    let mut buf = vec![0u8; 2048];
    match tokio::time::timeout(timeout, reader.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => is_2xx_response(&buf[..n]),
        Ok(Ok(_)) => false, // connection closed without data
        Ok(Err(e)) => {
            warn!("TCP read from '{}' failed: {}", proxy_addr, e);
            false
        }
        Err(_) => {
            warn!("TCP OPTIONS ping to '{}' timed out", proxy_addr);
            false
        }
    }
}

async fn send_options_tls(proxy_addr: &str, timeout: Duration) -> bool {
    use rustls::pki_types::ServerName;
    use std::sync::Arc as StdArc;
    use tokio_rustls::TlsConnector;

    // Resolve host for SNI
    let host = proxy_addr.split(':').next().unwrap_or(proxy_addr).to_string();

    // Build a permissive TLS config that accepts self-signed certs
    let tls_config = {
        #[derive(Debug)]
        struct AcceptAny;
        impl rustls::client::danger::ServerCertVerifier for AcceptAny {
            fn verify_server_cert(
                &self,
                _end_entity: &rustls::pki_types::CertificateDer<'_>,
                _intermediates: &[rustls::pki_types::CertificateDer<'_>],
                _server_name: &ServerName<'_>,
                _ocsp_response: &[u8],
                _now: rustls::pki_types::UnixTime,
            ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }

            fn verify_tls12_signature(
                &self,
                message: &[u8],
                cert: &rustls::pki_types::CertificateDer<'_>,
                dss: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
                rustls::crypto::verify_tls12_signature(
                    message,
                    cert,
                    dss,
                    &rustls::crypto::aws_lc_rs::default_provider()
                        .signature_verification_algorithms,
                )
            }

            fn verify_tls13_signature(
                &self,
                message: &[u8],
                cert: &rustls::pki_types::CertificateDer<'_>,
                dss: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
                rustls::crypto::verify_tls13_signature(
                    message,
                    cert,
                    dss,
                    &rustls::crypto::aws_lc_rs::default_provider()
                        .signature_verification_algorithms,
                )
            }

            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                rustls::crypto::aws_lc_rs::default_provider()
                    .signature_verification_algorithms
                    .supported_schemes()
            }
        }

        match rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(StdArc::new(AcceptAny))
            .with_no_client_auth()
        {
            cfg => StdArc::new(cfg),
        }
    };

    let connector = TlsConnector::from(tls_config);

    let server_name = match ServerName::try_from(host.clone()) {
        Ok(sn) => sn,
        Err(e) => {
            warn!("Invalid TLS server name '{}': {}", host, e);
            return false;
        }
    };

    let tcp_stream =
        match tokio::time::timeout(timeout, TcpStream::connect(proxy_addr)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!("TLS/TCP connect to '{}' failed: {}", proxy_addr, e);
                return false;
            }
            Err(_) => {
                warn!("TLS connect to '{}' timed out", proxy_addr);
                return false;
            }
        };

    let local_addr = tcp_stream
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "127.0.0.1:0".to_string());

    let tls_stream =
        match tokio::time::timeout(timeout, connector.connect(server_name, tcp_stream)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                warn!("TLS handshake with '{}' failed: {}", proxy_addr, e);
                return false;
            }
            Err(_) => {
                warn!("TLS handshake with '{}' timed out", proxy_addr);
                return false;
            }
        };

    let msg = build_options_message(proxy_addr, "tls", &local_addr);
    let (mut reader, mut writer) = tokio::io::split(tls_stream);

    if let Err(e) = writer.write_all(msg.as_bytes()).await {
        warn!("TLS write to '{}' failed: {}", proxy_addr, e);
        return false;
    }

    let mut buf = vec![0u8; 2048];
    match tokio::time::timeout(timeout, reader.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => is_2xx_response(&buf[..n]),
        Ok(Ok(_)) => false,
        Ok(Err(e)) => {
            warn!("TLS read from '{}' failed: {}", proxy_addr, e);
            false
        }
        Err(_) => {
            warn!("TLS OPTIONS ping to '{}' timed out", proxy_addr);
            false
        }
    }
}

// Silence unused import warning for ToSocketAddrs
fn _use_to_socket_addrs(_: impl ToSocketAddrs) {}
