//! Pure audio constants and helpers shared by the kernel HDA driver and its
//! host tests (spec 0030, ADR 0063).
//!
//! Nothing here touches hardware. The command encoding and the volume mapping
//! are the two pieces that are easy to get subtly wrong (a 12-bit verb carries
//! an 8-bit payload, a 4-bit style verb a 16-bit one), so they live in a
//! crate the host test suite already exercises.

/// 48 kHz, 16-bit, two channels (`AC_FMT_*`: channels-1 = 1, bits = 1,
/// 48 kHz base).
pub const FORMAT_48K_16_2: u16 = 0x0011;

/// 44.1 kHz, 16-bit, two channels: the same channel and width fields with the
/// 44.1 kHz base bit (14) set.
pub const FORMAT_44K_16_2: u16 = 0x4011;

/// The codec format word for an application rate, or `None` when the driver
/// does not program it (spec 0030, A8; ADR 0064).
pub const fn format_for(rate: u32) -> Option<u16> {
    match rate {
        44_100 => Some(FORMAT_44K_16_2),
        48_000 => Some(FORMAT_48K_16_2),
        _ => None,
    }
}

/// The sample ring: 8 periods of 4 KiB (`CBL = 32768`).
pub const RING_BYTES: usize = 32 * 1024;
/// One buffer descriptor entry, and one accounting unit.
pub const PERIOD_BYTES: usize = 4096;
/// Buffer descriptor entries in the ring.
pub const PERIODS: usize = 8;

/// The controller stream tag the DAC converter is bound to. Zero is avoided
/// because an unprogrammed descriptor reads tag 0.
pub const STREAM_TAG: u32 = 1;

/// Builds one 32-bit HDA command word.
///
/// Layout: codec address bits 31..28, node id bits 27..20, verb bits 19..8,
/// payload bits 15..0. The HDA split is decided by the verb's top three bits:
/// `0b111` selects an 8-bit payload (12-bit verb) and everything else a 16-bit
/// payload (the "4-bit" verbs such as `SET_STREAM_FORMAT` and
/// `SET_AMP_GAIN_MUTE`). Because every 12-bit verb used here has a zero low
/// byte, the same shift works for both: a payload of at most `0xff` simply
/// lands in the low byte.
///
/// A payload above `0xff` is only meaningful for a verb below `0x700`; a
/// caller that passes a wide payload to a 12-bit verb would corrupt the verb
/// field, so that is a programming error.
pub const fn command(cad: u8, nid: u8, verb: u16, payload: u16) -> u32 {
    ((cad as u32) << 28) | (((nid & 0x7f) as u32) << 20) | ((verb as u32) << 8) | (payload as u32)
}

/// Maps an application volume in `0..=100` onto a codec amplifier's step
/// range (`0..=step_count`). The codec's step size is in quarter-dB units and
/// is irrelevant here: the application sees a percentage, the amp sees a step.
///
/// A percentage above 100 is clamped; the caller validates before calling.
pub const fn gain_for_percent(percent: u8, step_count: u16) -> u8 {
    let percent = if percent > 100 { 100 } else { percent } as u32;
    ((percent * step_count as u32) / 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_layout_matches_the_hda_split() {
        // 12-bit verb, 8-bit payload: GET_PARAMETER(nid 2, WIDGET_CAP).
        assert_eq!(command(0, 2, 0xf00, 0x09), 0x002_f0009);
        // 12-bit verb whose low byte is nonzero: SET_CHANNEL_STREAMID.
        assert_eq!(command(0, 2, 0x706, 0x10), 0x002_70610);
        // 16-bit payload: SET_STREAM_FORMAT 48 kHz/16-bit/stereo.
        assert_eq!(command(0, 2, 0x200, FORMAT_48K_16_2), 0x002_20011);
        // 16-bit payload: output amp, left channel, gain 74.
        assert_eq!(command(0, 2, 0x300, 0xa04a), 0x002_3a04a);
        // Codec address and node id occupy their own fields.
        assert_eq!(command(1, 0x7f, 0xf00, 0x00), 0x1_7f_f0000);
    }

    #[test]
    fn the_top_three_verb_bits_select_the_payload_width() {
        // The codec decoder in QEMU branches on this mask; keep the constants
        // we rely on inside the expected half. `0x700..=0x7ff` and
        // `0xf00..=0xfff` are 12/8; everything else is 4/16 (including
        // GET_AMP_GAIN_MUTE at 0xb00, whose payload needs 16 bits).
        for verb in [0x200u16, 0x300, 0x706, 0x707, 0xb00, 0xf00, 0xf02] {
            let word = command(0, 1, verb, 0);
            let twelve_bit = (word & 0x70000) == 0x70000;
            assert_eq!(twelve_bit, (verb & 0x700) == 0x700, "verb {verb:#05x}");
        }
    }

    #[test]
    fn volume_maps_across_the_amp_range() {
        assert_eq!(gain_for_percent(0, 74), 0);
        assert_eq!(gain_for_percent(50, 74), 37);
        assert_eq!(gain_for_percent(100, 74), 74);
        assert_eq!(gain_for_percent(101, 74), 74);
        assert_eq!(gain_for_percent(100, 0), 0);
    }

    #[test]
    fn the_ring_is_whole_periods() {
        assert_eq!(RING_BYTES, PERIODS * PERIOD_BYTES);
    }

    #[test]
    fn format_words_select_the_two_supported_rates() {
        assert_eq!(format_for(44_100), Some(FORMAT_44K_16_2));
        assert_eq!(format_for(48_000), Some(FORMAT_48K_16_2));
        for unsupported in [0, 8_000, 22_050, 32_000, 88_200, 96_000] {
            assert_eq!(format_for(unsupported), None, "rate {unsupported}");
        }

        // Low seven bits: channels-1 = 1 and 16-bit = 1 for both words.
        for word in [FORMAT_44K_16_2, FORMAT_48K_16_2] {
            assert_eq!(word & 0x0f, 0x1);
            assert_eq!((word >> 4) & 0x7, 0x1);
            assert_eq!((word >> 15) & 0x1, 0x0, "PCM, not non-PCM");
        }
        // Only the 44.1 kHz base bit (14) differs.
        assert_eq!(FORMAT_44K_16_2 ^ FORMAT_48K_16_2, 1 << 14);
    }
}
