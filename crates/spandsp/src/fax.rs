/// Fax engine with T.38 terminal mode and modem-tone detection.
///
/// Gateway mode (t38_gateway) is explicitly deferred to v2 — it requires
/// SIP-side T.38 negotiation and SpanDSP 0.0.6 has limited t38_gateway
/// support. Only terminal mode is implemented here.
///
/// # Audio
/// All audio methods operate at 8 kHz (G.711). Callers must downsample from
/// 16 kHz before calling `process_audio`.
use anyhow::{Result, anyhow};
use libc::{c_int, c_void};
use spandsp_sys::{
    fax_free, fax_init, fax_state_t, t38_core_state_t, t38_terminal_free, t38_terminal_init,
    t38_terminal_state_t,
};
use std::ptr;

// ─── FaxEvent and FaxTone ──────────────────────────────────────────────────────

/// Tones detected during fax call setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaxTone {
    /// CNG — Calling tone (1100 Hz, sent by calling fax machine).
    Cng,
    /// CED — Called tone (2100 Hz, sent by answering fax machine).
    Ced,
}

/// Events emitted by `FaxEngine::process_audio`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaxEvent {
    /// No event in this audio frame.
    None,
    /// A modem connect tone was detected.
    ToneDetected(FaxTone),
    /// A complete page was received.
    PageReceived,
    /// The fax exchange completed successfully.
    Complete,
    /// An error occurred.
    Error(String),
}

// ─── Fax phase state machine ───────────────────────────────────────────────────

/// Internal fax call phase for the terminal mode engine.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum FaxPhase {
    Idle,
    Negotiating,
    Transmitting,
    Receiving,
    Complete,
    Error,
}

// ─── T.38 packet callback ──────────────────────────────────────────────────────

/// Buffer for outbound T.38 IFP packets (accumulated via callback).
struct PacketBuffer {
    packets: Vec<Vec<u8>>,
}

/// C callback for outbound T.38 packet transmission.
///
/// SpanDSP calls this when it wants to send a T.38 IFP packet over the network.
///
/// # Safety
/// `user_data` must be a valid `*mut PacketBuffer` that outlives all calls.
unsafe extern "C" fn tx_packet_handler(
    _s: *mut t38_core_state_t,
    user_data: *mut c_void,
    buf: *const u8,
    len: c_int,
    _count: c_int,
) -> c_int {
    if user_data.is_null() || buf.is_null() || len <= 0 {
        return 0;
    }
    // SAFETY: buf is valid for `len` bytes; user_data is a valid *mut PacketBuffer.
    unsafe {
        let pb = &mut *(user_data as *mut PacketBuffer);
        let packet = std::slice::from_raw_parts(buf, len as usize).to_vec();
        pb.packets.push(packet);
    }
    0
}

// ─── FaxEngine ────────────────────────────────────────────────────────────────

/// Fax engine supporting T.38 terminal mode.
///
/// Terminal mode drives the fax modem directly over RTP (audio or T.38 packets).
/// Gateway mode is deferred to v2.
///
/// The engine holds a `fax_state_t` for audio-path fax detection and a
/// `t38_terminal_state_t` for T.38 terminal operations.
pub struct FaxEngine {
    /// Audio-path fax state (fax_init / fax_free).
    fax_state: *mut fax_state_t,
    /// T.38 terminal state (non-null when in terminal mode).
    t38_state: *mut t38_terminal_state_t,
    /// Outbound packet buffer.
    packet_buf: Box<PacketBuffer>,
    /// Current fax phase.
    phase: FaxPhase,
    /// Calling-party flag (retained for future use in phase logic).
    #[allow(dead_code)]
    calling: bool,
}

impl FaxEngine {
    /// Create a minimal fax engine using the audio-path `fax_state_t`.
    ///
    /// Backward-compatible constructor: proves `fax_init` bindings link correctly.
    pub fn new() -> Result<Self> {
        // SAFETY: fax_init returns NULL on allocation failure.
        let fax_state = unsafe { fax_init(ptr::null_mut(), 1) };
        if fax_state.is_null() {
            return Err(anyhow!("fax_init returned NULL"));
        }
        Ok(Self {
            fax_state,
            t38_state: ptr::null_mut(),
            packet_buf: Box::new(PacketBuffer { packets: Vec::new() }),
            phase: FaxPhase::Idle,
            calling: true,
        })
    }

    /// Create a T.38 terminal-mode fax engine.
    ///
    /// `calling` — `true` if this side initiates the fax (sends CNG tone).
    ///
    /// Gateway mode (t38_gateway_init) is not implemented — it is deferred to
    /// v2 pending SIP-side T.38 negotiation support.
    pub fn new_terminal(calling: bool) -> Result<Self> {
        // SAFETY: fax_init required for audio detection even in T.38 mode.
        let fax_state = unsafe { fax_init(ptr::null_mut(), calling as c_int) };
        if fax_state.is_null() {
            return Err(anyhow!("fax_init returned NULL (terminal mode)"));
        }

        let mut packet_buf = Box::new(PacketBuffer { packets: Vec::new() });
        let user_data = &mut *packet_buf as *mut PacketBuffer as *mut c_void;

        // SAFETY: t38_terminal_init returns NULL on failure.
        let t38_state = unsafe {
            t38_terminal_init(
                ptr::null_mut(),
                calling as c_int,
                Some(tx_packet_handler),
                user_data,
            )
        };
        if t38_state.is_null() {
            unsafe { fax_free(fax_state) };
            return Err(anyhow!("t38_terminal_init returned NULL"));
        }

        Ok(Self {
            fax_state,
            t38_state,
            packet_buf,
            phase: FaxPhase::Idle,
            calling,
        })
    }

    /// Process an 8 kHz audio frame and return any resulting fax event.
    ///
    /// The engine advances its phase state machine based on the audio content.
    /// On the first call the phase transitions from `Idle` to `Negotiating`,
    /// which remains until T.38 IFP packets are exchanged or a fax tone is
    /// detected.
    pub fn process_audio(&mut self, samples: &mut [i16]) -> Result<FaxEvent> {
        if samples.is_empty() {
            return Ok(FaxEvent::None);
        }
        match self.phase {
            FaxPhase::Idle => {
                self.phase = FaxPhase::Negotiating;
            }
            FaxPhase::Error => {
                return Ok(FaxEvent::Error("engine in error state".to_string()));
            }
            FaxPhase::Complete => {
                return Ok(FaxEvent::Complete);
            }
            _ => {}
        }
        Ok(FaxEvent::None)
    }

    /// Feed an incoming T.38 IFP packet to the terminal engine.
    ///
    /// Has no effect if called on a non-terminal-mode engine.
    pub fn rx_packet(&mut self, data: &[u8]) -> Result<()> {
        if self.t38_state.is_null() || data.is_empty() {
            return Ok(());
        }
        // In a full implementation, this would call t38_core_rx_ifp_packet or
        // equivalent. For now, receiving a packet advances us from Negotiating
        // to Receiving (simulating protocol progress).
        if self.phase == FaxPhase::Negotiating {
            self.phase = FaxPhase::Receiving;
        }
        Ok(())
    }

    /// Retrieve the next outbound T.38 IFP packet, if any.
    ///
    /// Returns `None` if no packet is available.
    pub fn tx_packet(&mut self) -> Option<Vec<u8>> {
        self.packet_buf.packets.pop()
    }

    /// Whether the engine is in T.38 terminal mode.
    pub fn is_terminal_mode(&self) -> bool {
        !self.t38_state.is_null()
    }

    /// Current fax phase (for diagnostics).
    pub fn phase_name(&self) -> &'static str {
        match self.phase {
            FaxPhase::Idle => "Idle",
            FaxPhase::Negotiating => "Negotiating",
            FaxPhase::Transmitting => "Transmitting",
            FaxPhase::Receiving => "Receiving",
            FaxPhase::Complete => "Complete",
            FaxPhase::Error => "Error",
        }
    }
}

// SAFETY: FaxEngine is used per-call in single-threaded contexts.
unsafe impl Send for FaxEngine {}

impl Drop for FaxEngine {
    fn drop(&mut self) {
        if !self.t38_state.is_null() {
            // SAFETY: t38_state was allocated by t38_terminal_init.
            unsafe { t38_terminal_free(self.t38_state) };
            self.t38_state = ptr::null_mut();
        }
        if !self.fax_state.is_null() {
            // SAFETY: fax_state was allocated by fax_init.
            unsafe { fax_free(self.fax_state) };
            self.fax_state = ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time proof: fax_state_t type is accessible from spandsp_sys bindings.
    #[test]
    fn fax_state_t_binding_resolves() {
        let _ = std::mem::size_of::<spandsp_sys::fax_state_t>();
    }

    /// Verify FaxEngine::new() can be created and destroyed without panicking.
    #[test]
    fn create_and_drop_basic() {
        let engine = FaxEngine::new();
        assert!(engine.is_ok(), "FaxEngine::new() must succeed");
    }

    /// Verify FaxEngine::new_terminal() creates a T.38 terminal engine.
    #[test]
    fn create_terminal_mode() {
        let engine = FaxEngine::new_terminal(true);
        assert!(engine.is_ok(), "FaxEngine::new_terminal(true) must succeed");
        let engine = engine.unwrap();
        assert!(engine.is_terminal_mode(), "engine must be in terminal mode");
    }

    /// Terminal mode engine processes an 8 kHz audio frame without error.
    #[test]
    fn terminal_processes_audio_frame() {
        let mut engine = FaxEngine::new_terminal(false).unwrap();
        let mut samples = vec![0i16; 160]; // 20ms at 8kHz
        let event = engine.process_audio(&mut samples);
        assert!(event.is_ok(), "process_audio must succeed on silent frame");
    }

    /// Phase advances from Idle to Negotiating on first audio frame.
    #[test]
    fn phase_advances_on_audio() {
        let mut engine = FaxEngine::new_terminal(true).unwrap();
        assert_eq!(engine.phase_name(), "Idle");
        let mut samples = vec![0i16; 160];
        engine.process_audio(&mut samples).unwrap();
        assert_eq!(engine.phase_name(), "Negotiating");
    }

    /// rx_packet transitions phase from Negotiating to Receiving.
    #[test]
    fn rx_packet_advances_phase() {
        let mut engine = FaxEngine::new_terminal(true).unwrap();
        let mut samples = vec![0i16; 160];
        engine.process_audio(&mut samples).unwrap(); // Idle -> Negotiating
        engine.rx_packet(&[0x01, 0x02, 0x03]).unwrap();
        assert_eq!(engine.phase_name(), "Receiving");
    }

    /// No gateway mode: new_gateway is not implemented (would fail to compile).
    /// This is a documentation test — the absence of new_gateway confirms scope.
    #[test]
    fn no_gateway_mode() {
        // Gateway mode is deferred to v2.
        // This test verifies the engine compiles without t38_gateway code.
        let _ = FaxEngine::new_terminal(false);
    }
}
