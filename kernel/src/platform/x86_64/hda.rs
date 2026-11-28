//! Intel HDA playback driver (spec 0030, ADR 0063).
//!
//! # What this driver does
//!
//! It brings up one HDA controller, discovers one audio-output converter (DAC)
//! and one output-capable pin complex, binds them to stream tag 1 on playback
//! descriptor `SD4`, and plays a static 32 KiB ring of 8 × 4 KiB periods. The
//! ring is cyclic: the engine loops it and the driver rewrites sample bytes in
//! place, so the BDL is written once.
//!
//! Codec handling is discovery-based: node ids, the DAC, the pin and the
//! amplifier range all come from `GET_PARAMETER`, never from a table of QEMU
//! or Realtek ids.
//!
//! # Facts that cost debugging time
//!
//! - **Playback descriptors are `SD4..=SD7`.** QEMU's `intel-hda` matches a
//!   codec output stream tag only against its four output descriptors
//!   (`st[4..8]`); programming `SD0` produces a valid-looking stream that never
//!   receives data.
//! - **The descriptor layout is not the obvious one.** `FORMAT` is at `0x12`
//!   and `BDLPL/BDLPU` at `0x18/0x1C`; `FIFOSIZE` occupies `0x10`.
//! - **The command format splits by verb.** A verb whose top three bits are
//!   `0b111` carries an 8-bit payload; every other verb carries 16 bits. The
//!   shared encoder lives in [`abi::audio::command`].
//! - **`SD_STS.BCIS` is a bit, not a counter.** QEMU pre-buffers 8 KiB when the
//!   stream starts, so two periods can complete before the first poll. Position
//!   comes from `SD_LPIB`; the completion bit is only cleared.
//!
//! # Polling
//!
//! There is no interrupt path in this milestone. [`Hda::poll_periods`] reads
//! `SD_LPIB` and converts the byte delta into whole periods. The caller
//! (`audio::service`) runs it once per scheduler service opportunity, far more
//! often than the 21 ms a 4 KiB period takes at 48 kHz (23 ms at 44.1 kHz).

use abi::audio::{
    command, format_for, gain_for_percent, FORMAT_48K_16_2, PERIODS, PERIOD_BYTES, RING_BYTES,
    STREAM_TAG,
};
use arch_x86_64::KERNEL_BASE;
use core::sync::atomic::{fence, Ordering};

use crate::pci::HdaController;

/// Controller registers (spec 0030, "QEMU/HDA facts").
const GCTL: u64 = 0x08;
const STATESTS: u64 = 0x0E;
const CORBLBASE: u64 = 0x40;
const CORBUBASE: u64 = 0x44;
const CORBWP: u64 = 0x48;
const CORBRP: u64 = 0x4A;
const CORBCTL: u64 = 0x4C;
const CORBSTS: u64 = 0x4D;
const RIRBLBASE: u64 = 0x50;
const RIRBUBASE: u64 = 0x54;
const RIRBWP: u64 = 0x58;
const RINTCNT: u64 = 0x5A;
const RIRBCTL: u64 = 0x5C;
const RIRBSTS: u64 = 0x5D;

/// `GCTL.CRST`: 0 resets the controller and every codec, 1 runs.
const GCTL_CRST: u32 = 1;
/// `CORBCTL.RUN` and `RIRBCTL` bits.
const CORBCTL_RUN: u8 = 1 << 1;
const RIRBCTL_IRQ_EN: u8 = 1 << 0;
const RIRBCTL_DMA_EN: u8 = 1 << 1;

/// Stream descriptor offsets from `0x80 + index * 0x20`.
const SD_BASE: u64 = 0x80;
const SD_STRIDE: u64 = 0x20;
const SD_CTL: u64 = 0x00;
const SD_STS: u64 = 0x03;
const SD_LPIB: u64 = 0x04;
const SD_CBL: u64 = 0x08;
const SD_LVI: u64 = 0x0C;
const SD_FORMAT: u64 = 0x12;
const SD_BDLPL: u64 = 0x18;
const SD_BDLPU: u64 = 0x1C;

const SD_CTL_SRST: u32 = 1 << 0;
const SD_CTL_RUN: u32 = 1 << 1;
const SD_CTL_IOCE: u32 = 1 << 2;
const SD_CTL_TAG_SHIFT: u32 = 20;
/// The clearable status bits: completion (bit 2) and the two errors.
const SD_STS_CLEAR: u8 = 0x1C;

/// The first playback descriptor. Descriptors 0–3 are capture in QEMU.
const PLAYBACK_SD: u8 = 4;

/// Codec verbs and parameters used by discovery and setup.
const VERB_PARAMETERS: u16 = 0xF00;
const VERB_GET_CONNECTION_LIST: u16 = 0xF02;
const VERB_SET_STREAM_FORMAT: u16 = 0x200;
const VERB_SET_AMP_GAIN_MUTE: u16 = 0x300;
const VERB_SET_STREAM_ID: u16 = 0x706;
const VERB_SET_PIN_WIDGET_CONTROL: u16 = 0x707;

const PARAM_NODE_COUNT: u16 = 0x04;
const PARAM_FUNCTION_TYPE: u16 = 0x05;
const PARAM_WIDGET_CAPABILITIES: u16 = 0x09;
const PARAM_PIN_CAPABILITIES: u16 = 0x0C;
const PARAM_CONNECTION_LIST_LENGTH: u16 = 0x0E;
const PARAM_AMP_OUT_CAPABILITIES: u16 = 0x12;

const WIDGET_TYPE_AUDIO_OUT: u32 = 0;
const WIDGET_TYPE_PIN: u32 = 4;
const WIDGET_TYPE_SHIFT: u32 = 20;
const PIN_CAP_OUTPUT: u32 = 1 << 4;
const FUNCTION_TYPE_AUDIO: u32 = 1;
const PIN_OUTPUT_ENABLE: u16 = 1 << 6;

/// Amp payload: output amplifier, index 0, and the two channel selectors.
const AMP_OUTPUT: u16 = 1 << 15;
const AMP_LEFT: u16 = 1 << 13;
const AMP_RIGHT: u16 = 1 << 12;
const AMP_MUTE: u16 = 1 << 7;
const AMP_GAIN_MASK: u16 = 0x7F;

/// A bounded command response wait. QEMU answers synchronously; real
/// controllers answer in microseconds.
const COMMAND_SPINS: u32 = 200_000;
/// A bounded reset wait.
const RESET_SPINS: u32 = 100_000;
/// How far the pin-to-DAC walk may recurse and how many nodes it may visit.
const PATH_DEPTH: u8 = 3;
const PATH_BUDGET: u32 = 48;

/// One 128-byte-aligned DMA buffer. The device requires 128-byte alignment for
/// the CORB, the RIRB and the BDL; the sample ring gets it for free.
#[repr(C, align(128))]
struct SampleRing {
    bytes: [u8; RING_BYTES],
}

impl SampleRing {
    const fn new() -> Self {
        Self {
            bytes: [0; RING_BYTES],
        }
    }
}

#[repr(C, align(128))]
struct Corb {
    entries: [u32; 256],
}

impl Corb {
    const fn new() -> Self {
        Self { entries: [0; 256] }
    }
}

#[repr(C, align(128))]
struct Rirb {
    /// Response in the low 32 bits, codec address and solicited flag above.
    entries: [u64; 256],
}

impl Rirb {
    const fn new() -> Self {
        Self { entries: [0; 256] }
    }
}

/// One buffer descriptor list entry, exactly as the device reads it.
#[repr(C)]
#[derive(Clone, Copy)]
struct BdlEntry {
    address: u64,
    length: u32,
    flags: u32,
}

impl BdlEntry {
    const fn new() -> Self {
        Self {
            address: 0,
            length: 0,
            flags: 0,
        }
    }
}

#[repr(C, align(128))]
struct Bdl {
    entries: [BdlEntry; PERIODS],
}

impl Bdl {
    const fn new() -> Self {
        Self {
            entries: [BdlEntry::new(); PERIODS],
        }
    }
}

/// The codec facts discovery found, copied out for the boot log after the
/// audio lock is released.
#[derive(Clone, Copy)]
pub struct Info {
    pub device: u16,
    pub codec_vendor: u32,
    pub codec_subsystem: u32,
    pub dac: u8,
    pub pin: u8,
    pub stream: u8,
    pub amp_offset: u8,
    pub amp_steps: u8,
}

/// The one HDA stream. All DMA memory lives inside this value, which sits in
/// the audio facade's static, so no buffer is ever copied or allocated.
#[repr(C, align(128))]
pub struct Hda {
    ready: bool,
    running: bool,
    mmio: u64,
    dac: u8,
    pin: u8,
    amp_offset: u8,
    amp_steps: u8,
    /// The codec format word: `0x0011` (48 kHz) or `0x4011` (44.1 kHz).
    format: u16,
    volume: u8,
    mute: bool,
    corb_wp: u8,
    rirb_rp: u8,
    lpib: u32,
    codec_vendor: u32,
    codec_subsystem: u32,
    device: u16,
    ring: SampleRing,
    corb: Corb,
    rirb: Rirb,
    bdl: Bdl,
}

impl Hda {
    /// A zeroed, not-ready stream. `const` so the facade's static needs no
    /// runtime construction and no 35 KiB stack copy.
    pub const fn empty() -> Self {
        Self {
            ready: false,
            running: false,
            mmio: 0,
            dac: 0,
            pin: 0,
            amp_offset: 0,
            amp_steps: 0,
            format: FORMAT_48K_16_2,
            volume: 0,
            mute: false,
            corb_wp: 0,
            rirb_rp: 0,
            lpib: 0,
            codec_vendor: 0,
            codec_subsystem: 0,
            device: 0,
            ring: SampleRing::new(),
            corb: Corb::new(),
            rirb: Rirb::new(),
            bdl: Bdl::new(),
        }
    }

    /// Resets the controller, starts the command rings, discovers the codec and
    /// programs the converter. `None` means audio is unavailable; the caller
    /// logs it once and every sound operation stays `IoError`.
    pub fn bring_up(&mut self, controller: &HdaController) -> Result<Info, &'static str> {
        self.mmio = controller.mmio_base;
        self.device = controller.device;
        self.format = FORMAT_48K_16_2;

        self.reset_controller().ok_or("controller reset")?;
        self.start_command_rings().ok_or("command rings")?;

        let (dac, pin) = self.discover().ok_or("codec discovery")?;
        self.dac = dac;
        self.pin = pin;

        let amp = self
            .send(dac, VERB_PARAMETERS, PARAM_AMP_OUT_CAPABILITIES)
            .ok_or("amplifier capabilities")?;
        self.amp_steps = ((amp >> 8) & 0x7F) as u8;
        self.amp_offset = (amp & 0x7F) as u8;

        // Output pin enabled, converter bound to the stream tag and format.
        self.send(pin, VERB_SET_PIN_WIDGET_CONTROL, PIN_OUTPUT_ENABLE)
            .ok_or("pin setup")?;
        self.send(dac, VERB_SET_STREAM_FORMAT, self.format)
            .ok_or("converter format")?;
        self.send(dac, VERB_SET_STREAM_ID, ((STREAM_TAG as u16) << 4) | 0)
            .ok_or("converter stream")?;

        self.stop_stream();
        self.reset_stream();
        self.program_bdl();

        self.ready = true;
        self.volume = 100;
        self.set_volume(100);

        Ok(Info {
            device: self.device,
            codec_vendor: self.codec_vendor,
            codec_subsystem: self.codec_subsystem,
            dac,
            pin,
            stream: PLAYBACK_SD,
            amp_offset: self.amp_offset,
            amp_steps: self.amp_steps,
        })
    }

    /// True once [`Hda::bring_up`] succeeded.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Starts the engine if it is not already running.
    pub fn start(&mut self) -> bool {
        if !self.ready || self.running {
            return self.ready;
        }
        // Rewriting the pointer fields before RUN is harmless and makes a
        // restart after a stop deterministic. Every offset is relative to the
        // descriptor base `0x80 + 4 * 0x20`.
        let base = self.stream_base();
        self.wr32(base + SD_CBL, RING_BYTES as u32);
        self.wr16(base + SD_LVI, (PERIODS - 1) as u16);
        self.wr16(base + SD_FORMAT, self.format);
        self.wr32(
            base + SD_CTL,
            (STREAM_TAG << SD_CTL_TAG_SHIFT) | SD_CTL_IOCE | SD_CTL_RUN,
        );
        self.lpib = 0;
        self.running = true;
        true
    }

    /// Stops the engine. The ring content and the counters stay where they are.
    pub fn stop(&mut self) {
        if !self.ready {
            return;
        }
        let base = self.stream_base();
        self.wr32(base + SD_CTL, 0);
        self.running = false;
    }

    /// Whole 4 KiB periods the engine has consumed since the previous call.
    ///
    /// The position delta is taken modulo `CBL` so a ring wrap is one period,
    /// not a huge backward jump. The remainder is kept in `lpib`, so partial
    /// periods accumulate instead of being lost.
    pub fn poll_periods(&mut self) -> u32 {
        if !self.ready || !self.running {
            return 0;
        }
        let base = self.stream_base();
        let cbl = RING_BYTES as u32;
        let position = self.rd32(base + SD_LPIB) % cbl;
        let delta = (position + cbl - self.lpib) % cbl;
        let periods = delta / PERIOD_BYTES as u32;
        if periods > 0 {
            self.lpib = (self.lpib + periods * PERIOD_BYTES as u32) % cbl;
        }

        // The completion bit is a latch, not a counter; clear it so a later
        // interrupt path (if added) does not see a stale event.
        let status = self.rd8(base + SD_STS);
        if status & SD_STS_CLEAR != 0 {
            self.wr8(base + SD_STS, status & SD_STS_CLEAR);
        }
        periods
    }

    /// A mutable view of one period, for the facade's ring write.
    pub fn period_mut(&mut self, index: usize) -> &mut [u8] {
        let start = (index % PERIODS) * PERIOD_BYTES;
        &mut self.ring.bytes[start..start + PERIOD_BYTES]
    }

    /// Copies a buffer into the ring at `offset`, wrapping once. The facade has
    /// already checked that the covered bytes are free, so nothing in flight is
    /// overwritten.
    pub fn write_ring(&mut self, offset: usize, bytes: &[u8]) {
        let start = offset % RING_BYTES;
        let first = (RING_BYTES - start).min(bytes.len());
        self.ring.bytes[start..start + first].copy_from_slice(&bytes[..first]);
        let rest = bytes.len() - first;
        if rest > 0 {
            self.ring.bytes[..rest].copy_from_slice(&bytes[first..]);
        }
    }

    /// Zeroes the whole ring (close and owner exit).
    pub fn zero_ring(&mut self) {
        self.ring.bytes.fill(0);
    }

    /// Zeroes one period. Called for the period the engine just consumed, so a
    /// later wrap plays silence instead of stale samples.
    pub fn zero_period(&mut self, index: usize) {
        self.period_mut(index).fill(0);
    }

    /// Sets both channels from an application percentage (`0..=100`).
    pub fn set_volume(&mut self, percent: u8) {
        if !self.ready {
            return;
        }
        self.volume = percent.min(100);
        if self.amp_steps == 0 {
            // The codec advertises no output amplifier: nothing to program.
            return;
        }
        let gain = gain_for_percent(self.volume, self.amp_steps as u16) as u16;
        let base = AMP_OUTPUT | (gain & AMP_GAIN_MASK) | if self.mute { AMP_MUTE } else { 0 };
        let _ = self.send(self.dac, VERB_SET_AMP_GAIN_MUTE, base | AMP_LEFT);
        let _ = self.send(self.dac, VERB_SET_AMP_GAIN_MUTE, base | AMP_RIGHT);
    }

    /// Mutes or unmutes both channels at the current volume.
    pub fn set_mute(&mut self, muted: bool) {
        if !self.ready {
            return;
        }
        self.mute = muted;
        let volume = self.volume;
        self.set_volume(volume);
    }

    /// Selects the stream rate before the first write (spec 0030, A8;
    /// ADR 0064): `44_100` or `48_000`, 16-bit stereo.
    ///
    /// A running stream is stopped first, the DAC is reprogrammed, and the
    /// format is stored for the next [`Hda::start`]. An unsupported rate or an
    /// unavailable device is `false`; the caller maps it to `Invalid`/`IoError`.
    pub fn set_rate(&mut self, rate: u32) -> bool {
        let Some(format) = format_for(rate) else {
            return false;
        };
        if !self.ready {
            return false;
        }
        self.stop();
        self.format = format;
        let _ = self.send(self.dac, VERB_SET_STREAM_FORMAT, format);
        let _ = self.send(self.dac, VERB_SET_STREAM_ID, (STREAM_TAG as u16) << 4);
        true
    }

    // ---- controller bring-up -------------------------------------------

    fn reset_controller(&mut self) -> Option<()> {
        let gctl = self.rd32(GCTL);
        self.wr32(GCTL, gctl & !GCTL_CRST);
        for _ in 0..RESET_SPINS {
            core::hint::spin_loop();
        }
        self.wr32(GCTL, self.rd32(GCTL) | GCTL_CRST);
        for _ in 0..RESET_SPINS {
            if self.rd32(GCTL) & GCTL_CRST != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if self.rd32(GCTL) & GCTL_CRST == 0 {
            return None;
        }

        // At least codec 0 must report present before a command is meaningful.
        if self.rd16(STATESTS) & 1 == 0 {
            return None;
        }
        Some(())
    }

    fn start_command_rings(&mut self) -> Option<()> {
        let corb = physical(&self.corb.entries as *const _ as u64);
        let rirb = physical(&self.rirb.entries as *const _ as u64);

        self.wr32(CORBLBASE, corb as u32);
        self.wr32(CORBUBASE, (corb >> 32) as u32);
        self.wr32(RIRBLBASE, rirb as u32);
        self.wr32(RIRBUBASE, (rirb >> 32) as u32);

        // Reset both pointers: write the reset bit, wait for the CORB read
        // pointer to acknowledge, then clear it. RIRBWP's reset is write-only
        // (QEMU and the spec both drop the bit), so it is just written.
        self.wr16(RIRBWP, 0x8000);
        self.wr16(RIRBWP, 0x0000);

        self.wr16(CORBRP, 0x8000);
        for _ in 0..COMMAND_SPINS {
            if self.rd16(CORBRP) & 0x8000 != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.wr16(CORBRP, 0x0000);

        self.wr16(CORBWP, 0);
        self.wr8(CORBSTS, 1);
        self.wr8(RIRBSTS, 0x05);
        self.wr8(RINTCNT, 1);

        self.corb_wp = 0;
        self.rirb_rp = 0;

        self.wr8(CORBCTL, CORBCTL_RUN);
        // Enable the response interrupt at the ring level as well: QEMU (and
        // the spec) stop processing the CORB once `RINTCNT` responses have
        // arrived, so the count is reset by clearing `RIRBSTS` after every
        // response. The controller's `INTCTL` stays 0, so no PCI interrupt is
        // raised; the ring-level bit only exists to make the count reachable.
        self.wr8(RIRBCTL, RIRBCTL_DMA_EN | RIRBCTL_IRQ_EN);
        Some(())
    }

    /// Sends one command and returns its response.
    ///
    /// One command is outstanding at a time, so the response is the RIRB entry
    /// at the freshly advanced write pointer (the device increments first, then
    /// writes). The CORB write pointer names the last entry written and the
    /// device starts at `read pointer + 1`, so the first command lands at index
    /// 1, not 0 — both pointers are 0 after reset. A timeout is `None`;
    /// discovery treats it as unavailable.
    fn send(&mut self, nid: u8, verb: u16, payload: u16) -> Option<u32> {
        let word = command(0, nid, verb, payload);
        self.corb_wp = self.corb_wp.wrapping_add(1);
        self.corb.entries[self.corb_wp as usize] = word;
        fence(Ordering::SeqCst);
        self.wr16(CORBWP, self.corb_wp as u16);

        for _ in 0..COMMAND_SPINS {
            let write_pointer = self.rd16(RIRBWP) as u8;
            if write_pointer != self.rirb_rp {
                fence(Ordering::SeqCst);
                self.rirb_rp = write_pointer;
                let response = self.rirb.entries[write_pointer as usize];
                // Clear the response status: this resets the controller's
                // response counter and re-arms CORB processing.
                self.wr8(RIRBSTS, 0x01);
                return Some(response as u32);
            }
            core::hint::spin_loop();
        }
        None
    }

    fn parameter(&mut self, nid: u8, parameter: u16) -> Option<u32> {
        self.send(nid, VERB_PARAMETERS, parameter)
    }

    /// Finds one audio function group, its first audio-output converter and an
    /// output pin whose connection walk reaches that converter.
    fn discover(&mut self) -> Option<(u8, u8)> {
        let root = self.parameter(0, PARAM_NODE_COUNT)?;
        let first = (root & 0xFF) as u8;
        let count = ((root >> 16) & 0xFF) as u8;

        let mut function_group = None;
        for nid in first..first.saturating_add(count) {
            let kind = self.parameter(nid, PARAM_FUNCTION_TYPE)?;
            if kind & 0xFF == FUNCTION_TYPE_AUDIO {
                function_group = Some(nid);
                break;
            }
        }
        let function_group = function_group?;

        let children = self.parameter(function_group, PARAM_NODE_COUNT)?;
        let first = (children & 0xFF) as u8;
        let count = ((children >> 16) & 0xFF) as u8;

        let mut dac = None;
        for nid in first..first.saturating_add(count) {
            let capabilities = self.parameter(nid, PARAM_WIDGET_CAPABILITIES)?;
            if (capabilities >> WIDGET_TYPE_SHIFT) & 0xF == WIDGET_TYPE_AUDIO_OUT {
                dac = Some(nid);
                break;
            }
        }
        let dac = dac?;

        // Prefer a pin that actually reaches the converter. If none does, the
        // codec is not one this driver can route; do not guess.
        let mut pin = None;
        for nid in first..first.saturating_add(count) {
            let capabilities = self.parameter(nid, PARAM_WIDGET_CAPABILITIES)?;
            if (capabilities >> WIDGET_TYPE_SHIFT) & 0xF != WIDGET_TYPE_PIN {
                continue;
            }
            let pin_caps = self.parameter(nid, PARAM_PIN_CAPABILITIES)?;
            if pin_caps & PIN_CAP_OUTPUT == 0 {
                continue;
            }
            let mut budget = PATH_BUDGET;
            if self.reaches(nid, dac, PATH_DEPTH, &mut budget) {
                pin = Some(nid);
                break;
            }
        }
        let pin = pin?;

        self.codec_vendor = self.parameter(0, 0x00)?;
        self.codec_subsystem = self.parameter(0, 0x01)?;
        Some((dac, pin))
    }

    /// Walks a widget's connection list looking for `target`.
    ///
    /// `depth` bounds the walk (pin → mixer → selector → DAC is the deepest
    /// layout this driver claims) and `budget` bounds the total node visits, so
    /// a pathological codec graph cannot spin.
    fn reaches(&mut self, from: u8, target: u8, depth: u8, budget: &mut u32) -> bool {
        if depth == 0 || *budget == 0 {
            return false;
        }
        *budget -= 1;
        let Some(length) = self.parameter(from, PARAM_CONNECTION_LIST_LENGTH) else {
            return false;
        };
        if length & 0x80 != 0 {
            // Long (16-bit) connection list entries are out of scope.
            return false;
        }
        let length = (length & 0x7F) as u8;
        let mut index = 0u8;
        while index < length {
            let Some(word) = self.send(from, VERB_GET_CONNECTION_LIST, index as u16) else {
                return false;
            };
            let chunk = 4.min(length - index);
            for slot in 0..chunk {
                let nid = ((word >> (8 * slot)) & 0xFF) as u8;
                if nid == target {
                    return true;
                }
                if nid != 0 && self.reaches(nid, target, depth - 1, budget) {
                    return true;
                }
            }
            index += chunk;
        }
        false
    }

    // ---- stream ---------------------------------------------------------

    fn stream_base(&self) -> u64 {
        SD_BASE + u64::from(PLAYBACK_SD) * SD_STRIDE
    }

    fn stop_stream(&mut self) {
        let base = self.stream_base();
        self.wr32(base + SD_CTL, 0);
    }

    fn reset_stream(&mut self) {
        let base = self.stream_base();
        self.wr32(base + SD_CTL, SD_CTL_SRST);
        for _ in 0..RESET_SPINS {
            core::hint::spin_loop();
        }
        self.wr32(base + SD_CTL, 0);
        for _ in 0..RESET_SPINS {
            core::hint::spin_loop();
        }
        self.wr8(base + SD_STS, SD_STS_CLEAR);
    }

    fn program_bdl(&mut self) {
        let ring = physical(&self.ring.bytes as *const _ as u64);
        for (index, entry) in self.bdl.entries.iter_mut().enumerate() {
            entry.address = ring + (index * PERIOD_BYTES) as u64;
            entry.length = PERIOD_BYTES as u32;
            entry.flags = 1; // IOC: completion on this entry.
        }
        let bdl = physical(&self.bdl.entries as *const _ as u64);
        let base = self.stream_base();
        self.wr32(base + SD_BDLPL, bdl as u32);
        self.wr32(base + SD_BDLPU, (bdl >> 32) as u32);
        self.wr32(base + SD_CBL, RING_BYTES as u32);
        self.wr16(base + SD_LVI, (PERIODS - 1) as u16);
        self.wr16(base + SD_FORMAT, self.format);
    }

    // ---- MMIO -----------------------------------------------------------

    fn rd32(&self, offset: u64) -> u32 {
        // SAFETY: the BAR was found and enabled by `pci::find_hda`; every
        // offset is a constant inside the 16 KiB window and the page is
        // identity-mapped device memory.
        unsafe { core::ptr::read_volatile((self.mmio + offset) as *const u32) }
    }

    fn wr32(&self, offset: u64, value: u32) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.mmio + offset) as *mut u32, value) }
    }

    fn rd16(&self, offset: u64) -> u16 {
        // SAFETY: as above.
        unsafe { core::ptr::read_volatile((self.mmio + offset) as *const u16) }
    }

    fn wr16(&self, offset: u64, value: u16) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.mmio + offset) as *mut u16, value) }
    }

    fn rd8(&self, offset: u64) -> u8 {
        // SAFETY: as above.
        unsafe { core::ptr::read_volatile((self.mmio + offset) as *const u8) }
    }

    fn wr8(&self, offset: u64, value: u8) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.mmio + offset) as *mut u8, value) }
    }
}

/// Physical address of a kernel static (ADR 0002 identity map).
fn physical(virtual_address: u64) -> u64 {
    debug_assert!(virtual_address >= KERNEL_BASE);
    virtual_address - KERNEL_BASE
}
