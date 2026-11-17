//! Correctly rounded decimal text → binary64 parsing (`core.parse_f64_into`).
//!
//! Spec 0003 fixes one grammar for float text: an optional sign, a mantissa
//! with at least one digit and an optional `.` group, an optional decimal
//! exponent, plus the four exact specials `inf`, `+inf`, `-inf` and `nan`.
//! Everything else is `Invalid`.
//!
//! The conversion itself is [`core`'s `f64: FromStr`](core::str::FromStr),
//! which is a correctly rounded decimal reader: it keeps at most 19
//! significant digits plus a sticky bit, tracks the decimal exponent exactly
//! and rounds the result half to even, including the subnormal range and the
//! overflow/underflow boundaries. It is allocation-free, which is why the
//! primitive can live in the interpreter rather than in `stdlib` language
//! code (a repeated ×/÷10 loop is one or more ulps off, see `Q41`).
//!
//! This module only adds the strict grammar: `FromStr` also accepts
//! `infinity`, case-insensitive `nan`/`inf` and a bare `+`/`-` sign, which the
//! language does not.

/// The bit pattern written for the literal `nan`.
///
/// The payload and sign of a quiet NaN are not observable through the
/// language (the formatters print `nan`, and comparisons are IEEE), so one
/// canonical positive quiet NaN keeps the result deterministic across cores.
pub const NAN_BITS: u64 = 0x7ff8_0000_0000_0000;

/// Parses spec 0003's float text into the nearest binary64 value, or `None`
/// for a malformed string.
///
/// The result is correctly rounded (round half to even) for every accepted
/// form: long mantissas keep their sticky bit, subnormals round to the right
/// subnormal, a magnitude above the binary64 range becomes `±Inf` and a
/// magnitude below it becomes `±0`. `-0.0` and `-inf` keep their sign.
pub fn parse(bytes: &[u8]) -> Option<f64> {
    match bytes {
        b"nan" => return Some(f64::from_bits(NAN_BITS)),
        b"inf" | b"+inf" => return Some(f64::INFINITY),
        b"-inf" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    if !grammar(bytes) {
        return None;
    }
    // The grammar accepted only ASCII, so this conversion cannot fail; the
    // `ok` keeps the function total rather than relying on that argument.
    core::str::from_utf8(bytes).ok()?.parse::<f64>().ok()
}

/// Spec 0003's grammar:
/// `[+-]? ( digits [ '.' digits? ] | '.' digits ) ( [eE] [+-]? digits )?`.
///
/// At least one mantissa digit is required, `1.` and `.5` are legal, and the
/// whole string must be consumed. No whitespace, no `infinity`, no `nan`
/// case variants, no embedded NUL and no non-ASCII byte can pass.
fn grammar(bytes: &[u8]) -> bool {
    let length = bytes.len();
    let mut at = 0;

    if at < length && (bytes[at] == b'+' || bytes[at] == b'-') {
        at += 1;
    }

    let mut digits = 0;
    while at < length && bytes[at].is_ascii_digit() {
        at += 1;
        digits += 1;
    }
    if at < length && bytes[at] == b'.' {
        at += 1;
        while at < length && bytes[at].is_ascii_digit() {
            at += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }

    if at < length && (bytes[at] == b'e' || bytes[at] == b'E') {
        at += 1;
        if at < length && (bytes[at] == b'+' || bytes[at] == b'-') {
            at += 1;
        }
        let start = at;
        while at < length && bytes[at].is_ascii_digit() {
            at += 1;
        }
        if at == start {
            return false;
        }
    }

    at == length
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(text: &[u8]) -> Option<u64> {
        parse(text).map(f64::to_bits)
    }

    #[test]
    fn exact_bits_for_known_values() {
        // Hard-coded expected bit patterns: these are the IEEE-754 binary64
        // results a correctly rounded reader must produce, independent of the
        // implementation's own arithmetic.
        let accepted: &[(&[u8], u64)] = &[
            (b"0", 0x0000_0000_0000_0000),
            (b"-0", 0x8000_0000_0000_0000),
            (b"1", 0x3ff0_0000_0000_0000),
            (b"-1", 0xbff0_0000_0000_0000),
            (b"1.", 0x3ff0_0000_0000_0000),
            (b"+2", 0x4000_0000_0000_0000),
            (b".5", 0x3fe0_0000_0000_0000),
            (b"2.5e-3", 0x3f64_7ae1_47ae_147b),
            (b"1E+2", 0x4059_0000_0000_0000),
            (b"0.1", 0x3fb9_9999_9999_999a),
            // The decimal literal is not exactly representable; the nearest
            // binary64 is the same one the compiler's literal reader picks.
            (b"2.675", 0x4005_6666_6666_6666),
            (b"1e23", 0x44b5_2d02_c7e1_4af6),
            // Repeated ×/÷10 scaling lands two ulps high here; the exact
            // reader is not (this is the Q41 correction the sample exercises).
            (b"1e-7", 0x3e7a_d7f2_9abc_af48),
            // 2^53 + 1 rounds down to even; 2^53 + 3 rounds to 2^53 + 4.
            (b"9007199254740993", 0x4340_0000_0000_0000),
            (b"9007199254740995", 0x4340_0000_0000_0002),
            // Halfway between 1 and the next binary64: ties to even (1).
            (
                b"1.00000000000000011102230246251565404236316680908203125",
                0x3ff0_0000_0000_0000,
            ),
            (
                b"1.0000000000000002220446049250313080847263336181640625",
                0x3ff0_0000_0000_0001,
            ),
            // The smallest normal and the largest subnormal, plus the texts
            // one decimal digit either side of the boundary.
            (b"2.2250738585072014e-308", 0x0010_0000_0000_0000),
            (b"2.225073858507201e-308", 0x000f_ffff_ffff_ffff),
            (b"2.2250738585072011e-308", 0x000f_ffff_ffff_ffff),
            (b"2.2250738585072012e-308", 0x0010_0000_0000_0000),
            // Smallest subnormal and the half-ulp boundary below it: the two
            // texts differ in the last digit of the exact half value.
            (b"4.9406564584124654e-324", 0x0000_0000_0000_0001),
            (b"2.4703282292062327e-324", 0x0000_0000_0000_0000),
            (b"2.4703282292062328e-324", 0x0000_0000_0000_0001),
            // Underflow and overflow saturate with `Ok`, keeping the sign.
            (b"1e-400", 0x0000_0000_0000_0000),
            (b"-1e-400", 0x8000_0000_0000_0000),
            (b"1e308", 0x7fe1_ccf3_85eb_c8a0),
            (b"1.7976931348623157e308", 0x7fef_ffff_ffff_ffff),
            (b"1.7976931348623158e308", 0x7fef_ffff_ffff_ffff),
            (b"1.7976931348623159e308", 0x7ff0_0000_0000_0000),
            (b"1e309", 0x7ff0_0000_0000_0000),
            (b"-1e309", 0xfff0_0000_0000_0000),
            (b"1e400", 0x7ff0_0000_0000_0000),
            // A 38-digit mantissa: correct rounding, not a 19-digit truncation.
            (
                b"1234567890123456789012345678901234567890e-40",
                0x3fbf_9add_3746_f65f,
            ),
            (b"inf", 0x7ff0_0000_0000_0000),
            (b"+inf", 0x7ff0_0000_0000_0000),
            (b"-inf", 0xfff0_0000_0000_0000),
            (b"nan", NAN_BITS),
        ];
        for (text, expected) in accepted {
            assert_eq!(
                bits(text),
                Some(*expected),
                "{}",
                core::str::from_utf8(text).unwrap_or("<bytes>")
            );
        }

        // The grammar's `digits '.' digits?` accepts a trailing point and a
        // leading point, and the exponent may carry a sign.
        assert_eq!(bits(b"1.e5"), Some(0x40f8_6a00_0000_0000));
        assert_eq!(bits(b".25e1"), Some(0x4004_0000_0000_0000));
        assert_eq!(bits(b"+.5"), Some(0x3fe0_0000_0000_0000));
        assert_eq!(bits(b"0e999999999999999999999"), Some(0));
    }

    #[test]
    fn malformed_text_is_rejected() {
        // Includes the forms `f64::from_str` itself accepts but the language
        // does not (`infinity`, case variants, `+nan`) and every prefix/suffix
        // and punctuation error.
        let rejected: &[&[u8]] = &[
            b"",
            b"+",
            b"-",
            b".",
            b"+.",
            b"e5",
            b"E5",
            b"1e",
            b"1E",
            b"1e+",
            b"1e-",
            b"1.2.3",
            b"1..",
            b".e5",
            b"1 2",
            b" 1",
            b"1 ",
            b"\t1",
            b"1\n",
            b"--1",
            b"++1",
            b"+-1",
            b"1,5",
            b"1_000",
            b"0x1p3",
            b"0x10",
            b"INF",
            b"Inf",
            b"NaN",
            b"NAN",
            b"+nan",
            b"-nan",
            b"nan ",
            b" infinity",
            b"infinity",
            b"infi",
            b"in",
            b"1e5x",
            b"1e 5",
            b"1e5.0",
            b"1f",
            b"1d",
            b"1.2e3e4",
            b"1\x00",
            b"\x001",
            b"1\x001",
            // Arabic-Indic digit one: valid UTF-8 but not an ASCII digit.
            "\u{0661}".as_bytes(),
            // Invalid UTF-8 and a stray continuation byte.
            b"1\xff",
            b"\xff",
            b"\x80",
            b"1\x80",
        ];
        for text in rejected {
            assert_eq!(bits(text), None, "{:?}", core::str::from_utf8(text));
        }

        // The permissive reader would have accepted these; the grammar is the
        // only reason they are `Invalid` here, so pin the contrast.
        for text in [&b"infinity"[..], b"NaN", b"+nan", b"-nan", b"INF"] {
            assert!(
                core::str::from_utf8(text)
                    .ok()
                    .and_then(|text| text.parse::<f64>().ok())
                    .is_some(),
                "the host reader accepts {:?}",
                core::str::from_utf8(text)
            );
        }
    }

    #[test]
    fn shortest_text_round_trips_to_the_same_bits() {
        // `f64short` is an exact base-10^9 implementation, independent of the
        // reader: formatting a value and parsing it back must reproduce the
        // identical bit pattern for every finite value and both infinities.
        let mut state = 0x243f_6a88_85a3_08d3u64;
        let mut checked = 0;
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state);
            if value.is_nan() {
                // The sign and payload of a NaN are not part of the language.
                continue;
            }
            let mut out = [0u8; crate::f64short::MAX_TEXT];
            let length = crate::f64short::format(value, &mut out);
            assert_eq!(
                bits(&out[..length]),
                Some(value.to_bits()),
                "{}",
                core::str::from_utf8(&out[..length]).expect("ASCII")
            );
            checked += 1;
        }
        assert!(checked > 19_000, "most patterns are finite");
    }

    #[test]
    fn very_long_mantissas_agree_with_the_core_reader() {
        // The grammar is the only filter; the conversion below it is core's
        // correctly rounded reader. Long inputs (hundreds of digits, tiny
        // exponents) must reach it whole rather than being truncated by the
        // primitive, so compare the bits with the host reader on the same
        // accepted text.
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for length in [1usize, 19, 20, 40, 200, 700] {
            for _ in 0..20 {
                let mut text = std::string::String::new();
                if state & 1 == 1 {
                    text.push('-');
                }
                for index in 0..length {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let digit = if index == 0 {
                        1 + (state % 9) as u8
                    } else {
                        (state % 10) as u8
                    };
                    text.push((b'0' + digit) as char);
                }
                text.push('e');
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let exponent = (state % 700) as i64 - 400;
                text.push_str(&std::format!("{}", exponent));

                let expected = text.parse::<f64>().expect("the host reader accepts it");
                assert_eq!(bits(text.as_bytes()), Some(expected.to_bits()), "{}", text);
            }
        }
    }
}
