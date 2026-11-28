//! The audio-output facade (spec 0030, ADR 0063).
//!
//! One machine-wide playback stream sits behind one [`SpinLock`]: the driver
//! owns the DMA ring, the facade owns the byte counters, the ownership and the
//! silence policy. `net/mod.rs` is the model: a static state value, a `service`
//! call from the scheduler, and no allocation.
//!
//! The engine is cyclic. The application writes whole blocks into the ring; the
//! facade accounts `written - consumed` bytes and zeroes each period as the
//! device passes it, so an underrun plays silence instead of replaying stale
//! samples.
//!
//! No other lock is taken while [`AUDIO`] is held. The boot log is printed after
//! the guard is dropped, and the self-test returns a summary instead of
//! printing from inside the lock.

use abi::audio::{PERIODS, PERIOD_BYTES, RING_BYTES};
use abi::Status;

use crate::hda::Hda;
use crate::println;
use crate::sync::SpinLock;

/// The one stream. `Hda::empty()` is `const`, so the whole 35 KiB state (ring,
/// CORB, RIRB, BDL) is static storage and never copied through the stack.
struct State {
    hda: Hda,
    /// The task that opened the stream, or `0` when it is closed.
    owner: u64,
    written: u64,
    consumed: u64,
    running: bool,
    underruns: u32,
    /// Application volume, `0..=100`; preserved across close/open.
    volume: u8,
    mute: bool,
}

impl State {
    const fn new() -> Self {
        Self {
            hda: Hda::empty(),
            owner: 0,
            written: 0,
            consumed: 0,
            running: false,
            underruns: 0,
            volume: 100,
            mute: false,
        }
    }

    /// Free space in the ring, always a multiple of one period while the
    /// stream is running.
    fn free(&self) -> usize {
        let queued = self.written.saturating_sub(self.consumed);
        (RING_BYTES as u64).saturating_sub(queued) as usize
    }
}

static AUDIO: SpinLock<State> = SpinLock::new(State::new());

/// Finds and brings up the controller. Called once at boot, before the
/// scheduler, so a device that is absent or unusable only costs one log line.
pub fn init() {
    let controller = crate::pci::find_hda();
    let result = {
        let mut state = AUDIO.lock();
        match controller {
            Some(controller) => state.hda.bring_up(&controller),
            None => Err("no controller"),
        }
    };

    match result {
        Ok(info) => println!(
            "audio: hda {:04x} codec {:08x}/{:08x} dac {} pin {} sd {} amp {} steps offset {}",
            info.device,
            info.codec_vendor,
            info.codec_subsystem,
            info.dac,
            info.pin,
            info.stream,
            info.amp_steps,
            info.amp_offset
        ),
        Err("no controller") => println!("audio: no HDA controller; sound disabled"),
        Err(stage) => println!("audio: hda present but unusable at {stage}; sound disabled"),
    }
}

/// The scheduler service opportunity: account every period the device has
/// consumed and silence it behind the playback pointer. A blocked writer is
/// woken after the space is published, so the retry sees it (spec 0030, C4).
pub fn service() {
    let consumed = {
        let mut state = AUDIO.lock();
        if !state.running || !state.hda.is_ready() {
            return;
        }
        let periods = state.hda.poll_periods();
        for _ in 0..periods {
            if state.written == state.consumed {
                state.underruns = state.underruns.saturating_add(1);
            }
            let index = ((state.consumed / PERIOD_BYTES as u64) as usize) % PERIODS;
            state.hda.zero_period(index);
            state.consumed = state.consumed.saturating_add(PERIOD_BYTES as u64);
        }
        periods > 0
    };
    if consumed {
        crate::task::wake_audio();
    }
}

// ---- application surface (spec 0030, C1–C4) ----------------------------

/// Opens the playback stream for `owner` (spec 0030, B1/C3). The format is
/// validated before the device is touched; a second task is `Full`; the owner
/// may reopen, which restarts at byte 0 with its volume and mute preserved.
pub fn open(rate: u64, channels: u64, bits: u64, owner: u64) -> Status {
    if owner == 0
        || channels != 2
        || bits != 16
        || abi::audio::format_for(rate as u32).is_none()
    {
        return Status::Invalid;
    }
    let mut state = AUDIO.lock();
    if !state.hda.is_ready() {
        return Status::IoError;
    }
    if state.owner != 0 && state.owner != owner {
        return Status::Full;
    }
    if !state.hda.set_rate(rate as u32) {
        return Status::IoError;
    }
    state.owner = owner;
    state.written = 0;
    state.consumed = 0;
    state.underruns = 0;
    state.running = false;
    let volume = state.volume;
    let muted = state.mute;
    state.hda.set_volume(volume);
    state.hda.set_mute(muted);
    Status::Ok
}

/// Enqueues the caller's whole buffer (spec 0030, B2). `WouldBlock` means the
/// ring has less free space than the buffer and the machine blocks/retries.
pub fn write(owner: u64, bytes: &[u8]) -> Result<(), Status> {
    let mut state = AUDIO.lock();
    if !state.hda.is_ready() {
        return Err(Status::IoError);
    }
    if state.owner == 0 || state.owner != owner {
        return Err(Status::Invalid);
    }
    if bytes.is_empty() {
        return Err(Status::Invalid);
    }
    if bytes.len() > RING_BYTES {
        return Err(Status::CapacityExceeded);
    }
    if state.free() < bytes.len() {
        return Err(Status::WouldBlock);
    }
    let at = (state.written % RING_BYTES as u64) as usize;
    state.hda.write_ring(at, bytes);
    state.written = state.written.saturating_add(bytes.len() as u64);
    if !state.running && state.written - state.consumed >= PERIOD_BYTES as u64 {
        if state.hda.start() {
            state.running = true;
        }
    }
    Ok(())
}

/// Stops the stream, zeroes the ring and clears the owner (spec 0030, B4).
pub fn close(owner: u64) -> Status {
    let mut state = AUDIO.lock();
    if !state.hda.is_ready() {
        return Status::IoError;
    }
    if state.owner == 0 || state.owner != owner {
        return Status::Invalid;
    }
    state.hda.stop();
    state.hda.zero_ring();
    state.owner = 0;
    state.written = 0;
    state.consumed = 0;
    state.running = false;
    Status::Ok
}

/// Sets both channels from `0..=100` (spec 0030, B6). Owner-only.
pub fn volume(owner: u64, percent: u64) -> Status {
    if percent > 100 {
        return Status::Invalid;
    }
    let mut state = AUDIO.lock();
    if !state.hda.is_ready() {
        return Status::IoError;
    }
    if state.owner == 0 || state.owner != owner {
        return Status::Invalid;
    }
    state.volume = percent as u8;
    let volume = state.volume;
    state.hda.set_volume(volume);
    Status::Ok
}

/// Mutes or unmutes both channels at the current volume (spec 0030, B6).
pub fn mute(owner: u64, on: u64) -> Status {
    if on > 1 {
        return Status::Invalid;
    }
    let mut state = AUDIO.lock();
    if !state.hda.is_ready() {
        return Status::IoError;
    }
    if state.owner == 0 || state.owner != owner {
        return Status::Invalid;
    }
    state.mute = on == 1;
    let muted = state.mute;
    state.hda.set_mute(muted);
    Status::Ok
}

/// Bytes enqueued but not yet consumed; no owner check (spec 0030, C2).
pub fn queued() -> u64 {
    let state = AUDIO.lock();
    if !state.hda.is_ready() {
        return 0;
    }
    state.written.saturating_sub(state.consumed)
}

/// True when the ring has any free byte. The scheduler probes this after a
/// failed write, before blocking, to close the wake race (spec 0030, C4).
pub fn has_space() -> bool {
    AUDIO.lock().free() > 0
}

/// Closes the stream if `owner` is the task that exits (spec 0030, B4). Called
/// from `sched::terminate`, so a crashed player cannot leave the device
/// replaying its ring.
pub fn owner_exit(owner: u64) {
    let mut state = AUDIO.lock();
    if owner == 0 || state.owner != owner {
        return;
    }
    if state.hda.is_ready() {
        state.hda.stop();
        state.hda.zero_ring();
    }
    state.owner = 0;
    state.written = 0;
    state.consumed = 0;
    state.running = false;
}

/// Boot self-test (`igis.audio_selftest=<rate>`, spec 0030, V1): stream three
/// rings of a 440 Hz square wave at `rate` (44100 or 48000) and report the
/// pieces a proof needs.
pub fn selftest(rate: u32) {
    match run_selftest(rate) {
        Some((periods, bytes, underruns)) => println!(
            "audio: selftest ok at {} Hz: {} periods, {} bytes, {} underruns",
            rate, periods, bytes, underruns
        ),
        None => println!("audio: selftest skipped (no usable device or rate)"),
    }
}

fn run_selftest(rate: u32) -> Option<(u32, u64, u32)> {
    let mut state = AUDIO.lock();
    if !state.hda.is_ready() || !state.hda.set_rate(rate) {
        return None;
    }

    state.written = 0;
    state.consumed = 0;
    state.underruns = 0;
    state.running = false;

    let mut phase = 0u32;
    let mut level = 1i32;
    let deadline = crate::clock::now_ns().saturating_add(5_000_000_000);
    let mut guard = 0u64;

    // Prime one full ring, then start, so the first poll cannot underrun.
    for _ in 0..PERIODS {
        queue_period(&mut state, &mut phase, &mut level, rate);
    }
    if !state.hda.start() {
        return None;
    }
    state.running = true;

    // Stream two more rings, refilling as the device frees periods.
    let mut extra = 2 * PERIODS;
    while extra > 0 && !expired(deadline, &mut guard) {
        if state.free() >= PERIOD_BYTES {
            queue_period(&mut state, &mut phase, &mut level, rate);
            extra -= 1;
        } else {
            consume(&mut state);
        }
    }

    // Let the last periods be handed to the device before stopping.
    while state.written > state.consumed && !expired(deadline, &mut guard) {
        consume(&mut state);
    }

    state.hda.stop();
    state.running = false;
    let periods = (state.consumed / PERIOD_BYTES as u64) as u32;
    Some((periods, state.consumed, state.underruns))
}

/// The self-test's bounded wait: wall clock, with a spin count as the backstop
/// for a boot where the timer is not ticking yet.
fn expired(deadline: u64, guard: &mut u64) -> bool {
    *guard = guard.saturating_add(1);
    *guard > 200_000_000 || crate::clock::now_ns() >= deadline
}

fn consume(state: &mut State) {
    let periods = state.hda.poll_periods();
    for _ in 0..periods {
        if state.written == state.consumed {
            state.underruns = state.underruns.saturating_add(1);
        }
        let index = ((state.consumed / PERIOD_BYTES as u64) as usize) % PERIODS;
        state.hda.zero_period(index);
        state.consumed = state.consumed.saturating_add(PERIOD_BYTES as u64);
    }
}

fn queue_period(state: &mut State, phase: &mut u32, level: &mut i32, rate: u32) {
    let index = ((state.written / PERIOD_BYTES as u64) as usize) % PERIODS;
    fill_tone(state.hda.period_mut(index), phase, level, rate);
    state.written = state.written.saturating_add(PERIOD_BYTES as u64);
}

/// Writes 16-bit/stereo little-endian frames: the same level on both channels,
/// toggling every `rate / (2 × 440)` frames, so one full square-wave period is
/// 440 Hz at either supported rate (54.5 frames at 48 kHz, 50.1 at 44.1 kHz).
fn fill_tone(bytes: &mut [u8], phase: &mut u32, level: &mut i32, rate: u32) {
    for frame in bytes.chunks_exact_mut(4) {
        *phase += 880;
        if *phase >= rate {
            *phase -= rate;
            *level = -*level;
        }
        let sample = if *level > 0 { 6000i16 } else { -6000i16 };
        let [low, high] = sample.to_le_bytes();
        frame[0] = low;
        frame[1] = high;
        frame[2] = low;
        frame[3] = high;
    }
}
