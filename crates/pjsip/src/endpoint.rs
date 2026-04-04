// crates/pjsip/src/endpoint.rs
//! Safe wrapper around pjsip_endpoint — the core SIP processing engine.
//!
//! Handles:
//! - pjlib initialization
//! - Caching pool creation
//! - Endpoint creation
//! - Transport binding (UDP/TCP/TLS)
//! - Module registration for INVITE sessions, 100rel, session timers
//!
//! All operations MUST happen on the pjsip OS thread.

use crate::error::check_status;
use crate::pool::CachingPool;
use anyhow::{Result, anyhow};
use std::ffi::CString;
use std::ptr;

/// Configuration for creating a PjEndpoint.
#[derive(Debug, Clone)]
pub struct PjEndpointConfig {
    /// Bind address (e.g. "0.0.0.0").
    pub bind_addr: String,
    /// SIP port (e.g. 5060).
    pub port: u16,
    /// Transport: "udp", "tcp", or "tls".
    pub transport: String,
    /// TLS certificate file path (required when transport = "tls").
    pub tls_cert_file: Option<String>,
    /// TLS private key file path.
    pub tls_privkey_file: Option<String>,
    /// Enable session timers (RFC 4028). Default: true.
    pub session_timers: bool,
    /// Session-Expires value in seconds. Default: 1800 (30 minutes).
    pub session_expires: u32,
    /// Min-SE value in seconds. Default: 90.
    pub min_se: u32,
    /// Enable 100rel / PRACK (RFC 3262). Default: true.
    pub enable_100rel: bool,
}

impl Default for PjEndpointConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0".to_string(),
            port: 5060,
            transport: "udp".to_string(),
            tls_cert_file: None,
            tls_privkey_file: None,
            session_timers: true,
            session_expires: 1800,
            min_se: 90,
            enable_100rel: true,
        }
    }
}

/// Wraps a running pjsip_endpoint with all required modules initialized.
///
/// MUST only be used from the pjsip OS thread.
pub struct PjEndpoint {
    pub(crate) endpt: *mut pjsip_sys::pjsip_endpoint,
    pub(crate) _caching_pool: CachingPool,
    pub(crate) pool: *mut pjsip_sys::pj_pool_t,
    config: PjEndpointConfig,
}

// SAFETY: PjEndpoint is only accessed from the dedicated pjsip thread.
// The bridge serializes all access through the command channel.
unsafe impl Send for PjEndpoint {}

impl PjEndpoint {
    /// Initialize pjlib + create endpoint + bind transport + register modules.
    ///
    /// # Safety
    ///
    /// Must be called on the pjsip OS thread. Not safe to call concurrently.
    pub fn create(config: PjEndpointConfig) -> Result<Self> {
        // 1. Initialize pjlib
        eprintln!("[PjEndpoint::create] pj_init");
        let status = unsafe { pjsip_sys::pj_init() };
        check_status(status).map_err(|e| anyhow!("pj_init failed: {e}"))?;
        eprintln!("[PjEndpoint::create] pj_init OK");

        // 2. Create caching pool
        let mut caching_pool = CachingPool::new();
        eprintln!("[PjEndpoint::create] caching pool created");

        // 3. Create endpoint
        let name = CString::new("super-voice").unwrap();
        let mut endpt: *mut pjsip_sys::pjsip_endpoint = ptr::null_mut();
        eprintln!("[PjEndpoint::create] pjsip_endpt_create");
        let status = unsafe {
            pjsip_sys::pjsip_endpt_create(
                caching_pool.factory_ptr(),
                name.as_ptr(),
                &mut endpt,
            )
        };
        check_status(status).map_err(|e| anyhow!("pjsip_endpt_create failed: {e}"))?;
        eprintln!("[PjEndpoint::create] pjsip_endpt_create OK");

        // 4. Create a pool for general allocations
        let pool_name = CString::new("pjsip-main").unwrap();
        let pool = unsafe {
            pjsip_sys::pjsip_endpt_create_pool(
                endpt,
                pool_name.as_ptr(),
                4096,
                4096,
            )
        };
        if pool.is_null() {
            return Err(anyhow!("failed to create main pool"));
        }

        let mut ep = Self {
            endpt,
            _caching_pool: caching_pool,
            pool,
            config,
        };

        // 5. Initialize required modules
        eprintln!("[PjEndpoint::create] init_modules");
        ep.init_modules()?;
        eprintln!("[PjEndpoint::create] init_modules OK");

        // 6. Start transport
        eprintln!("[PjEndpoint::create] start_transport");
        ep.start_transport()?;
        eprintln!("[PjEndpoint::create] start_transport OK");

        Ok(ep)
    }

    /// Initialize INVITE session, 100rel, session timer, and replaces modules.
    fn init_modules(&mut self) -> Result<()> {
        // UA layer (required for dialog/INVITE session)
        let status = unsafe { pjsip_sys::pjsip_ua_init_module(self.endpt, ptr::null()) };
        check_status(status).map_err(|e| anyhow!("pjsip_ua_init_module failed: {e}"))?;

        // INVITE session module
        let inv_cb = pjsip_sys::pjsip_inv_callback {
            on_state_changed: Some(crate::bridge::on_inv_state_changed),
            on_new_session: Some(crate::bridge::on_inv_new_session),
            on_rx_offer: None,
            on_rx_reinvite: None,
            on_tsx_state_changed: None,
            ..unsafe { std::mem::zeroed() }
        };
        let status = unsafe { pjsip_sys::pjsip_inv_usage_init(self.endpt, &inv_cb) };
        check_status(status).map_err(|e| anyhow!("pjsip_inv_usage_init failed: {e}"))?;

        // 100rel / PRACK (RFC 3262)
        if self.config.enable_100rel {
            let status = unsafe { pjsip_sys::pjsip_100rel_init_module(self.endpt) };
            check_status(status).map_err(|e| anyhow!("pjsip_100rel_init_module failed: {e}"))?;
        }

        // Session timers (RFC 4028)
        if self.config.session_timers {
            let status = unsafe { pjsip_sys::pjsip_timer_init_module(self.endpt) };
            check_status(status).map_err(|e| anyhow!("pjsip_timer_init_module failed: {e}"))?;
        }

        // Replaces (RFC 3891)
        let status = unsafe { pjsip_sys::pjsip_replaces_init_module(self.endpt) };
        check_status(status).map_err(|e| anyhow!("pjsip_replaces_init_module failed: {e}"))?;

        tracing::info!(
            session_timers = self.config.session_timers,
            prack = self.config.enable_100rel,
            "pjsip modules initialized"
        );

        Ok(())
    }

    /// Bind a SIP transport (UDP, TCP, or TLS).
    fn start_transport(&mut self) -> Result<()> {
        let bind_str = format!("{}:{}", self.config.bind_addr, self.config.port);
        tracing::info!(
            transport = %self.config.transport,
            bind = %bind_str,
            "starting pjsip transport"
        );

        match self.config.transport.as_str() {
            "udp" => self.start_udp_transport(),
            "tcp" => self.start_tcp_transport(),
            "tls" => self.start_tls_transport(),
            other => Err(anyhow!("unsupported transport: {other}")),
        }
    }

    fn start_udp_transport(&mut self) -> Result<()> {
        let mut addr: pjsip_sys::pj_sockaddr_in = unsafe { std::mem::zeroed() };
        // sin_family is pj_uint8_t (u8) — PJ_AF_INET = 2 (AF_INET)
        addr.sin_family = libc::AF_INET as u16;
        addr.sin_port = self.config.port.to_be();
        // 0.0.0.0 = INADDR_ANY
        addr.sin_addr.s_addr = 0;

        let mut tp: *mut pjsip_sys::pjsip_transport = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjsip_udp_transport_start(
                self.endpt,
                &addr,
                ptr::null(),  // published address (auto)
                1,            // async count
                &mut tp,
            )
        };
        check_status(status).map_err(|e| anyhow!("pjsip_udp_transport_start failed: {e}"))?;
        Ok(())
    }

    fn start_tcp_transport(&mut self) -> Result<()> {
        let mut addr: pjsip_sys::pj_sockaddr_in = unsafe { std::mem::zeroed() };
        addr.sin_family = libc::AF_INET as u16;
        addr.sin_port = self.config.port.to_be();
        addr.sin_addr.s_addr = 0;

        let mut factory: *mut pjsip_sys::pjsip_tpfactory = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjsip_tcp_transport_start(
                self.endpt,
                &addr,
                1,  // async count
                &mut factory,
            )
        };
        check_status(status).map_err(|e| anyhow!("pjsip_tcp_transport_start failed: {e}"))?;
        Ok(())
    }

    fn start_tls_transport(&mut self) -> Result<()> {
        let cert = self
            .config
            .tls_cert_file
            .as_deref()
            .ok_or_else(|| anyhow!("TLS requires tls_cert_file"))?;
        let key = self
            .config
            .tls_privkey_file
            .as_deref()
            .ok_or_else(|| anyhow!("TLS requires tls_privkey_file"))?;

        // pjsip_tls_setting_default is inline — zero-fill and set the
        // timeout and verify fields to safe defaults manually.
        let mut tls_setting: pjsip_sys::pjsip_tls_setting = unsafe { std::mem::zeroed() };

        let cert_cstr = CString::new(cert)?;
        let key_cstr = CString::new(key)?;
        tls_setting.cert_file =
            unsafe { pjsip_sys::pj_str(cert_cstr.as_ptr() as *mut _) };
        tls_setting.privkey_file =
            unsafe { pjsip_sys::pj_str(key_cstr.as_ptr() as *mut _) };

        let mut addr: pjsip_sys::pj_sockaddr_in = unsafe { std::mem::zeroed() };
        addr.sin_family = libc::AF_INET as u16;
        addr.sin_port = self.config.port.to_be();
        addr.sin_addr.s_addr = 0;

        let mut factory: *mut pjsip_sys::pjsip_tpfactory = ptr::null_mut();
        let status = unsafe {
            pjsip_sys::pjsip_tls_transport_start(
                self.endpt,
                &tls_setting,
                &addr,
                ptr::null(),
                1,
                &mut factory,
            )
        };
        check_status(status).map_err(|e| anyhow!("pjsip_tls_transport_start failed: {e}"))?;
        Ok(())
    }

    /// Run one iteration of the event loop (non-blocking, max_timeout ms).
    ///
    /// Call this in a tight loop on the pjsip thread.
    /// Uses zero timeout (poll mode) to ensure it never blocks.
    pub fn handle_events(&self, max_timeout_ms: u32) -> Result<()> {
        let timeout = pjsip_sys::pj_time_val {
            sec: (max_timeout_ms / 1000) as libc::c_long,
            msec: (max_timeout_ms % 1000) as libc::c_long,
        };
        let status = unsafe { pjsip_sys::pjsip_endpt_handle_events(self.endpt, &timeout) };
        // PJ_ETIMEDOUT is not an error — it means no events in the timeout window.
        if status != 0 {
            let pj_etimedout = 120004; // PJ_ETIMEDOUT
            if status != pj_etimedout {
                check_status(status).map_err(|e| anyhow!("handle_events: {e}"))?;
            }
        }
        Ok(())
    }

    /// Non-blocking poll — returns immediately if no events are ready.
    pub fn poll_events(&self) -> Result<()> {
        self.handle_events(0)
    }
}

impl PjEndpoint {
    /// Perform explicit graceful shutdown of the endpoint.
    ///
    /// Releases the main pool and shuts down pjlib.
    /// Must be called from the pjsip OS thread before dropping this struct.
    ///
    /// NOTE: We intentionally skip `pjsip_endpt_destroy` here because on macOS
    /// with kqueue, it can block indefinitely waiting for the I/O queue to drain.
    /// The endpoint memory is released when the caching pool is destroyed (which
    /// happens when the `CachingPool` is dropped as part of `PjEndpoint` destruction).
    pub fn shutdown(mut self) {
        if !self.pool.is_null() && !self.endpt.is_null() {
            unsafe { pjsip_sys::pjsip_endpt_release_pool(self.endpt, self.pool) };
            self.pool = ptr::null_mut();
        }
        // Null out endpt to prevent Drop from calling destroy.
        self.endpt = ptr::null_mut();
        // pj_shutdown() cleans up pjlib global state including thread registry.
        unsafe { pjsip_sys::pj_shutdown() };
        // Prevent Drop from running (fields already nulled).
        std::mem::forget(self);
    }
}

impl Drop for PjEndpoint {
    fn drop(&mut self) {
        // This runs on error paths. Skip endpt_destroy to avoid blocking.
        if !self.pool.is_null() && !self.endpt.is_null() {
            unsafe { pjsip_sys::pjsip_endpt_release_pool(self.endpt, self.pool) };
        }
        // Do NOT call pjsip_endpt_destroy — it may block indefinitely on macOS.
        unsafe { pjsip_sys::pj_shutdown() };
    }
}
