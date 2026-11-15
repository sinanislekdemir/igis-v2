//! Shortest round-trip decimal text for `f64` (spec 0003, "Floating point";
//! Q41).
//!
//! [`format`] writes the shortest digit string that parses back to the same
//! IEEE-754 bits, in the shape spec 0003 defines. The work is exact, not
//! approximate: the value is expanded to its full decimal form and the
//! round-trip interval is tested with integer comparisons, so the digits are
//! the true shortest ones rather than a good approximation of them.
//!
//! # Why exact
//!
//! A decimal `d` parses back to the binary value `v` exactly when `d` lies in
//! `v`'s rounding interval: `[v - ulp/2, v + ulp/2]`, with both endpoints
//! included exactly when `v`'s significand is even (round-half-to-even), and
//! the lower half-gap halved again at a power-of-two boundary. Every binary64
//! value and both of its interval endpoints are dyadic rationals, so they have
//! finite decimal expansions. Representing all of them over one common power
//! of ten turns the question into integer arithmetic:
//!
//! - `v = m * 2^e`.
//! - `L` is the smallest of `{e, lower_exp, upper_exp, 0}`; the factor
//!   `2^(-L)` clears every binary denominator, and `k = -L` decimal places
//!   clear the remaining factor of five (`2^-k = 5^k / 10^k`).
//! - The three numbers become integers `vd`, `lower = vd - lo` and
//!   `upper = vd + up`, all divided by `10^k`.
//!
//! The integers are held in little-endian base-`10^9` limbs, so multiplying by
//! `5^27` or `2^30` (one `u64` chunk) and dropping `10^q` digits (a limb shift
//! plus one small division) are cheap. `5^27` is the largest power of five that
//! fits a `u64`, and a limb times it still fits a `u128`.
//!
//! The result of the expansion has at most 769 decimal digits (the smallest
//! subnormal multiplies `5^1075` by two), which is 86 limbs; [`LIMBS`] keeps
//! two spare. The shortest form itself is at most 17 digits, the published
//! bound for binary64: 17 significant decimal digits always round-trip.
//!
//! # Picking the digits
//!
//! For `n` significant digits, with `d` the exact digit count of `vd`, the
//! candidates are `c * 10^q` with `q = d - n`. The two candidates that bracket
//! `vd` are `c0 = vd / 10^q` and `c0 + 1`; if any `n`-digit decimal lies in the
//! interval then one of those two does (the interval is convex and contains
//! `vd`). The first `n` for which one of them passes is the shortest digit
//! count; when both pass, the one nearer `vd` wins, with an exact tie going to
//! the even last digit.
//!
//! No allocation, no floating-point arithmetic after the initial decode, and
//! no host call.

use core::cmp::Ordering;

/// Maximum significant digits in a shortest binary64 form.
pub(crate) const MAX_DIGITS: usize = 17;

/// Longest text [`format`] can produce. The longest positional form is
/// `-0.` plus five zeros plus 17 digits (25 bytes); the longest scientific form
/// is a sign, 17 digits, `.`, `e`, a minus sign and three exponent digits (23).
/// The value is rounded up to 32, and the machine's staging array is exactly
/// this size.
pub(crate) const MAX_TEXT: usize = 32;

/// Limbs of a [`Big`]. 88 limbs hold 792 decimal digits, above the 769 the
/// widest expansion needs.
const LIMBS: usize = 88;

/// The limb radix: nine decimal digits per limb, so a limb is a short decimal
/// slice and the digits never have to be extracted by division into a second
/// base.
const BASE: u64 = 1_000_000_000;

/// Decimal digits per limb.
const LIMB_DIGITS: usize = 9;

/// `10^0 ..= 10^8`, for splitting a limb into digits.
const POW10: [u64; LIMB_DIGITS] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
];

/// `5^27`, the largest power of five below `u64::MAX`.
const POW5_27: u64 = 7_450_580_596_923_828_125;

/// Where the dropped digits of a division fell relative to half a unit in the
/// last kept place.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Half {
    /// Below half: round down.
    Below,
    /// Exactly half: round to even.
    Exactly,
    /// Above half: round up.
    Above,
}

/// A non-negative integer in little-endian base-[`BASE`] limbs.
#[derive(Clone, Copy)]
struct Big {
    limb: [u64; LIMBS],
    len: usize,
}

impl Big {
    const fn zero() -> Self {
        Big {
            limb: [0; LIMBS],
            len: 0,
        }
    }

    fn from_u64(value: u64) -> Self {
        let mut out = Big::zero();
        let mut rest = value;
        while rest > 0 {
            out.limb[out.len] = rest % BASE;
            rest /= BASE;
            out.len += 1;
        }
        out
    }

    /// Drops leading zero limbs, so `len == 0` is exactly zero.
    fn normalize(&mut self) {
        while self.len > 0 && self.limb[self.len - 1] == 0 {
            self.len -= 1;
        }
    }

    /// `self *= factor` for a `u64` factor below [`POW5_27`]. The product of a
    /// limb and the factor fits a `u128`, and so does the carry.
    fn mul_small(&mut self, factor: u64) {
        if factor == 0 || self.len == 0 {
            self.len = 0;
            return;
        }
        let mut carry = 0u128;
        for index in 0..self.len {
            let product = self.limb[index] as u128 * factor as u128 + carry;
            self.limb[index] = (product % BASE as u128) as u64;
            carry = product / BASE as u128;
        }
        while carry > 0 {
            debug_assert!(self.len < LIMBS, "wide decimal expansion overflow");
            self.limb[self.len] = (carry % BASE as u128) as u64;
            carry /= BASE as u128;
            self.len += 1;
        }
    }

    /// `self *= 2^exponent`, in `u64` chunks.
    fn mul_pow2(&mut self, exponent: u32) {
        let mut rest = exponent;
        while rest >= 30 {
            self.mul_small(1 << 30);
            rest -= 30;
        }
        if rest > 0 {
            self.mul_small(1 << rest);
        }
    }

    /// `self *= 5^exponent`, in `u64` chunks.
    fn mul_pow5(&mut self, exponent: u32) {
        let mut rest = exponent;
        while rest >= 27 {
            self.mul_small(POW5_27);
            rest -= 27;
        }
        let mut factor = 1u64;
        for _ in 0..rest {
            factor *= 5;
        }
        if factor > 1 {
            self.mul_small(factor);
        }
    }

    fn cmp(&self, other: &Big) -> Ordering {
        if self.len != other.len {
            return self.len.cmp(&other.len);
        }
        for index in (0..self.len).rev() {
            match self.limb[index].cmp(&other.limb[index]) {
                Ordering::Equal => {}
                other => return other,
            }
        }
        Ordering::Equal
    }

    fn add(&self, other: &Big) -> Big {
        let mut out = Big::zero();
        let len = self.len.max(other.len);
        let mut carry = 0u64;
        for index in 0..len {
            let sum = self.limb[index] + other.limb[index] + carry;
            if sum >= BASE {
                out.limb[index] = sum - BASE;
                carry = 1;
            } else {
                out.limb[index] = sum;
                carry = 0;
            }
        }
        out.len = len;
        if carry > 0 {
            out.limb[out.len] = carry;
            out.len += 1;
        }
        out
    }

    /// `self - other`, which the caller guarantees is non-negative.
    fn sub(&self, other: &Big) -> Big {
        let mut out = Big::zero();
        let mut borrow = 0u64;
        for index in 0..self.len {
            let minuend = self.limb[index];
            let subtrahend = other.limb[index] + borrow;
            if minuend >= subtrahend {
                out.limb[index] = minuend - subtrahend;
                borrow = 0;
            } else {
                out.limb[index] = minuend + BASE - subtrahend;
                borrow = 1;
            }
        }
        out.len = self.len;
        debug_assert_eq!(borrow, 0, "negative decimal expansion");
        out.normalize();
        out
    }

    /// Number of decimal digits, with zero reported as one digit.
    fn digits(&self) -> usize {
        if self.len == 0 {
            return 1;
        }
        let mut digits = (self.len - 1) * LIMB_DIGITS;
        let mut top = self.limb[self.len - 1];
        loop {
            digits += 1;
            top /= 10;
            if top == 0 {
                return digits;
            }
        }
    }

    /// The decimal digit at `position`, counted from the least significant
    /// digit. Positions at or past the top of the number are zero.
    fn digit_at(&self, position: usize) -> u8 {
        let index = position / LIMB_DIGITS;
        if index >= self.len {
            return 0;
        }
        ((self.limb[index] / POW10[position % LIMB_DIGITS]) % 10) as u8
    }

    /// True when any of the low `count` decimal digits is non-zero.
    fn any_low_digit(&self, count: usize) -> bool {
        let limbs = count / LIMB_DIGITS;
        let extra = count % LIMB_DIGITS;
        for index in 0..limbs.min(self.len) {
            if self.limb[index] != 0 {
                return true;
            }
        }
        if extra > 0 && limbs < self.len && self.limb[limbs] % POW10[extra] != 0 {
            return true;
        }
        false
    }

    /// `floor(self / 10^digits)` and the rounding position of the dropped part.
    ///
    /// The quotient is produced by a limb shift and one division by
    /// `10^(digits % 9)`; the dropped digits are inspected digit by digit only
    /// when their leading digit is exactly five.
    fn div_pow10(&self, digits: usize) -> (Big, Half) {
        let shift = digits / LIMB_DIGITS;
        let extra = digits % LIMB_DIGITS;
        let mut out = Big::zero();
        if extra == 0 {
            for index in shift..self.len {
                out.limb[index - shift] = self.limb[index];
            }
            out.len = self.len.saturating_sub(shift);
        } else {
            let divisor = POW10[extra];
            let mut remainder = 0u64;
            for index in (shift..self.len).rev() {
                let current = remainder * BASE + self.limb[index];
                out.limb[index - shift] = current / divisor;
                remainder = current % divisor;
            }
            out.len = self.len.saturating_sub(shift);
        }
        out.normalize();

        let leading = self.digit_at(digits - 1);
        let half = match leading.cmp(&5) {
            Ordering::Greater => Half::Above,
            Ordering::Less => Half::Below,
            Ordering::Equal => {
                if self.any_low_digit(digits - 1) {
                    Half::Above
                } else {
                    Half::Exactly
                }
            }
        };
        (out, half)
    }

    fn to_u64(&self) -> u64 {
        let mut value = 0u64;
        for index in (0..self.len).rev() {
            value = value * BASE + self.limb[index];
        }
        value
    }
}

/// `value * 10^places` for a `value` below `10^18`.
fn scaled(value: u64, places: usize) -> Big {
    let mut out = Big::from_u64(value);
    let shift = places / LIMB_DIGITS;
    let extra = places % LIMB_DIGITS;
    if extra > 0 {
        out.mul_small(POW10[extra]);
    }
    if shift > 0 {
        for index in (0..out.len).rev() {
            out.limb[index + shift] = out.limb[index];
        }
        for index in 0..shift {
            out.limb[index] = 0;
        }
        out.len += shift;
    }
    out
}

/// Decimal digits of `value`, most significant first.
fn decimal_digits(value: u64, count: usize, out: &mut [u8; MAX_DIGITS + 1]) {
    let mut rest = value;
    for index in (0..count).rev() {
        out[index] = b'0' + (rest % 10) as u8;
        rest /= 10;
    }
}

/// The shortest round-trip digits of a finite, non-zero `value`.
///
/// Returns `(digits, count, e10)`: the significant digits most significant
/// first, their count (trailing zeros already stripped) and the scientific
/// exponent, so the value is `d[0].d[1..] * 10^e10`.
fn shortest(value: f64) -> (u64, usize, i32) {
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7FF) as i32;
    let fraction = bits & 0x000F_FFFF_FFFF_FFFF;
    let (mantissa, exponent) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), biased - 1075)
    };
    let even = mantissa & 1 == 0;

    // Half a unit in the last place, on each side. A power of two above the
    // smallest normal has half the gap below that it has above.
    let upper_exp = exponent - 1;
    let lower_exp = if biased > 1 && fraction == 0 {
        exponent - 2
    } else {
        exponent - 1
    };
    // Clear every binary denominator, then every factor of five. `value_shift`
    // is at most two when the value has a binary denominator, because the
    // smallest of the three exponents is never more than two below the value's
    // own.
    let low = exponent.min(lower_exp).min(upper_exp).min(0);
    let places = (-low) as u32;
    let value_shift = (exponent - low) as u32;
    let upper_shift = (upper_exp - low) as u32;
    let lower_shift = (lower_exp - low) as u32;

    // `five = 5^places`; when `places` is zero it stays one.
    let mut five = Big::from_u64(1);
    five.mul_pow5(places);

    // vd = mantissa * 2^value_shift * 5^places
    let vd = if value_shift <= 2 {
        let mut out = five;
        out.mul_small(mantissa << value_shift);
        out
    } else {
        debug_assert_eq!(places, 0, "a wide value has no decimal scaling");
        let mut out = Big::from_u64(mantissa);
        out.mul_pow2(value_shift);
        out
    };
    // up and lo are the half-gaps, on the same `10^places` scale.
    let up = if upper_shift <= 2 {
        let mut out = five;
        out.mul_small(1 << upper_shift);
        out
    } else {
        let mut out = Big::from_u64(1);
        out.mul_pow2(upper_shift);
        out
    };
    let lo = if lower_shift <= 2 {
        let mut out = five;
        out.mul_small(1 << lower_shift);
        out
    } else {
        let mut out = Big::from_u64(1);
        out.mul_pow2(lower_shift);
        out
    };

    let lower = vd.sub(&lo);
    let upper = vd.add(&up);
    let exact = vd.digits();

    let mut candidate = 0u64;
    let mut count = 0usize;
    let mut exponent10 = 0i32;

    for digits in 1..=MAX_DIGITS {
        if digits >= exact {
            // The exact expansion already has at most `digits` significant
            // digits, so it round-trips as it stands.
            candidate = vd.to_u64();
            count = exact;
            exponent10 = exact as i32 - 1 - places as i32;
            break;
        }
        let places_dropped = exact - digits;
        let (quotient, half) = vd.div_pow10(places_dropped);
        let down = quotient.to_u64();
        let up_value = down + 1;
        let down_inside = inside(&lower, &upper, down, places_dropped, even);
        let up_inside = inside(&lower, &upper, up_value, places_dropped, even);
        if down_inside || up_inside {
            let round_up = match half {
                Half::Above => true,
                Half::Below => false,
                // An exact tie rounds up, away from zero. Both candidates are
                // equally near, so either round-trips; the host reference
                // (`{:e}`) rounds up, and spec 0003 records that choice.
                Half::Exactly => true,
            };
            candidate = if down_inside && up_inside {
                if round_up {
                    up_value
                } else {
                    down
                }
            } else if up_inside {
                up_value
            } else {
                down
            };
            count = decimal_count(candidate);
            exponent10 = count as i32 - 1 + places_dropped as i32 - places as i32;
            break;
        }
    }
    // 17 digits always round-trip (the published binary64 bound), so the loop
    // always breaks. The fallback keeps a hypothetical arithmetic mistake from
    // running off the end of the digit buffer; the property tests rule it out.
    if count == 0 {
        candidate = vd.to_u64();
        count = exact.min(MAX_DIGITS);
        exponent10 = count as i32 - 1 - places as i32;
    }

    let mut buffer = [b'0'; MAX_DIGITS + 1];
    decimal_digits(candidate, count, &mut buffer);
    while count > 1 && buffer[count - 1] == b'0' {
        count -= 1;
    }
    let mut packed = 0u64;
    for index in 0..count {
        packed = packed * 10 + (buffer[index] - b'0') as u64;
    }
    (packed, count, exponent10)
}

/// Number of decimal digits of `value` (`0` included as one digit).
fn decimal_count(value: u64) -> usize {
    let mut count = 1;
    let mut rest = value;
    while rest >= 10 {
        rest /= 10;
        count += 1;
    }
    count
}

/// True when `value * 10^places` lies in the round-trip interval.
///
/// `even` is the significand's low bit: an even significand makes both interval
/// endpoints parse back to it, because a decimal exactly at the midpoint ties
/// to the even neighbour.
fn inside(lower: &Big, upper: &Big, value: u64, places: usize, even: bool) -> bool {
    let point = scaled(value, places);
    let above_lower = match point.cmp(lower) {
        Ordering::Greater => true,
        Ordering::Equal => even,
        Ordering::Less => false,
    };
    let below_upper = match point.cmp(upper) {
        Ordering::Less => true,
        Ordering::Equal => even,
        Ordering::Greater => false,
    };
    above_lower && below_upper
}

/// Writes the shortest round-trip text of `value` into `out` and returns its
/// length.
///
/// The caller guarantees that `out` has room for [`MAX_TEXT`] bytes. `NaN` is
/// `nan`, the infinities are `inf`/`-inf`, a zero is `0` or `-0`, and every
/// other value is the shortest significant-digit string in positional form for
/// `-6 < e10 + 1 <= 21` and in scientific form otherwise.
pub(crate) fn format(value: f64, out: &mut [u8]) -> usize {
    let bits = value.to_bits();
    let negative = bits >> 63 != 0;
    if value.is_nan() {
        out[..3].copy_from_slice(b"nan");
        return 3;
    }
    if value.is_infinite() {
        if negative {
            out[0] = b'-';
            out[1..4].copy_from_slice(b"inf");
            return 4;
        }
        out[..3].copy_from_slice(b"inf");
        return 3;
    }
    let mut at = 0;
    if negative {
        out[0] = b'-';
        at = 1;
    }
    if value == 0.0 {
        out[at] = b'0';
        return at + 1;
    }

    let (packed, count, exponent10) = shortest(value);
    let mut digits = [b'0'; MAX_DIGITS + 1];
    decimal_digits(packed, count, &mut digits);

    // Scientific form when the decimal point would sit outside `-6 < k <= 21`;
    // that is the ECMAScript shape, so `1e20` stays positional and `1e21` does
    // not, and `1e-7` goes scientific while `1e-6` does not.
    let point = exponent10 + 1;
    if point > -6 && point <= 21 {
        if point <= 0 {
            out[at] = b'0';
            out[at + 1] = b'.';
            at += 2;
            for _ in point..0 {
                out[at] = b'0';
                at += 1;
            }
            out[at..at + count].copy_from_slice(&digits[..count]);
            at += count;
        } else if (point as usize) < count {
            let split = point as usize;
            out[at..at + split].copy_from_slice(&digits[..split]);
            out[at + split] = b'.';
            out[at + split + 1..at + split + 1 + count - split]
                .copy_from_slice(&digits[split..count]);
            at += count + 1;
        } else {
            out[at..at + count].copy_from_slice(&digits[..count]);
            at += count;
            for _ in count..point as usize {
                out[at] = b'0';
                at += 1;
            }
        }
        return at;
    }

    out[at] = digits[0];
    at += 1;
    if count > 1 {
        out[at] = b'.';
        out[at + 1..at + count].copy_from_slice(&digits[1..count]);
        at += count;
    }
    out[at] = b'e';
    at += 1;
    let mut magnitude = exponent10;
    if magnitude < 0 {
        out[at] = b'-';
        at += 1;
        magnitude = -magnitude;
    }
    let mut buffer = [b'0'; 3];
    let mut width = 0;
    let mut rest = magnitude as u32;
    loop {
        buffer[width] = b'0' + (rest % 10) as u8;
        width += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for index in (0..width).rev() {
        out[at] = buffer[index];
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: f64) -> std::string::String {
        let mut out = [0u8; MAX_TEXT];
        let len = format(value, &mut out);
        std::string::String::from(core::str::from_utf8(&out[..len]).expect("ASCII"))
    }

    /// The reference shape rule applied to Rust's shortest digits.
    fn reference(value: f64) -> std::string::String {
        if value.is_nan() {
            return "nan".into();
        }
        if value.is_infinite() {
            return if value < 0.0 {
                "-inf".into()
            } else {
                "inf".into()
            };
        }
        let mut digits = std::string::String::new();
        let mut exponent = 0i32;
        if value == 0.0 {
            digits.push('0');
        } else {
            let scientific = std::format!("{:e}", value.abs());
            let (mantissa, exp) = scientific.split_once('e').expect("exponent");
            exponent = exp.parse().expect("exponent is an integer");
            digits = mantissa.replace('.', "");
        }
        let negative = value.is_sign_negative();
        render(negative, &digits, exponent)
    }

    fn render(negative: bool, digits: &str, exponent10: i32) -> std::string::String {
        let mut text = std::string::String::new();
        if negative {
            text.push('-');
        }
        let count = digits.len() as i32;
        let point = exponent10 + 1;
        if point > -6 && point <= 21 {
            if point <= 0 {
                text.push_str("0.");
                for _ in point..0 {
                    text.push('0');
                }
                text.push_str(digits);
            } else if point < count {
                text.push_str(&digits[..point as usize]);
                text.push('.');
                text.push_str(&digits[point as usize..]);
            } else {
                text.push_str(digits);
                for _ in count..point {
                    text.push('0');
                }
            }
        } else {
            text.push_str(&digits[..1]);
            if digits.len() > 1 {
                text.push('.');
                text.push_str(&digits[1..]);
            }
            text.push('e');
            if exponent10 < 0 {
                text.push('-');
            }
            text.push_str(&std::format!("{}", exponent10.abs()));
        }
        text
    }

    #[test]
    fn known_values() {
        assert_eq!(text(0.0), "0");
        assert_eq!(text(-0.0), "-0");
        assert_eq!(text(1.0), "1");
        assert_eq!(text(-1.0), "-1");
        assert_eq!(text(0.1), "0.1");
        assert_eq!(text(0.5), "0.5");
        assert_eq!(text(1.5), "1.5");
        assert_eq!(text(1.0 / 3.0), "0.3333333333333333");
        assert_eq!(text(1e20), "100000000000000000000");
        assert_eq!(text(1e21), "1e21");
        assert_eq!(text(1e-6), "0.000001");
        assert_eq!(text(1e-7), "1e-7");
        assert_eq!(text(5e-324), "5e-324");
        assert_eq!(text(f64::MAX), "1.7976931348623157e308");
        assert_eq!(text(f64::MIN_POSITIVE), "2.2250738585072014e-308");
        assert_eq!(text(9007199254740992.0), "9007199254740992");
        assert_eq!(text(9007199254740993.0), "9007199254740992");
        assert_eq!(text(0.30000000000000004), "0.30000000000000004");
        assert_eq!(text(f64::NAN), "nan");
        assert_eq!(text(f64::INFINITY), "inf");
        assert_eq!(text(f64::NEG_INFINITY), "-inf");
    }

    #[test]
    fn matches_the_reference_on_a_sample() {
        let mut state = 0x243F_6A88_85A3_08D3u64;
        for _ in 0..2000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state);
            if value.is_nan() || value.is_infinite() {
                continue;
            }
            assert_eq!(text(value), reference(value), "{value:?} bits {state:#x}");
        }
    }
}
