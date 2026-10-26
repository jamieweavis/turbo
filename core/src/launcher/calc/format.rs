//! Number formatting. Uses `.` for decimals and `,` for grouping regardless of locale: the
//! parser reads the same convention back, so a copied result can be typed in again.

use super::parser::Radix;

/// Up to `significant` significant digits, trailing zeros trimmed, never fewer than the
/// integer digits; scientific notation for very large or small magnitudes.
pub(super) fn significant(value: f64, significant: usize, group: bool) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if !(1e-6..1e15).contains(&magnitude) {
        let formatted = format!("{:.*e}", significant.saturating_sub(1), value);
        let (mantissa, exponent) = formatted.split_once('e').unwrap_or((&formatted, "0"));
        return format!("{}e{exponent}", trim_zeros(mantissa));
    }
    let integer_digits = magnitude.log10().floor() as i32 + 1;
    let decimals = (significant as i32 - integer_digits).clamp(0, 15) as usize;
    let formatted = trim_zeros(&format!("{value:.decimals$}"));
    let formatted = if formatted == "-0" {
        "0".into()
    } else {
        formatted
    };
    if group {
        grouped(&formatted)
    } else {
        formatted
    }
}

/// Exactly `decimals` decimal places (money).
pub(super) fn fixed(value: f64, decimals: usize, group: bool) -> String {
    let formatted = format!("{value:.decimals$}");
    let formatted = if formatted
        .trim_start_matches('-')
        .chars()
        .all(|c| c == '0' || c == '.')
    {
        formatted.trim_start_matches('-').to_owned()
    } else {
        formatted
    };
    if group {
        grouped(&formatted)
    } else {
        formatted
    }
}

/// An integer in another base: `0xFF`, `0b1010`, `0o17`, or plain decimal.
pub(super) fn radix(value: f64, radix: Radix) -> Option<String> {
    if value.fract() != 0.0 || value.abs() > 2f64.powi(53) {
        return None;
    }
    let n = value as i64;
    let (sign, n) = if n < 0 {
        ("-", n.unsigned_abs())
    } else {
        ("", n as u64)
    };
    Some(match radix {
        Radix::Decimal => format!("{sign}{n}"),
        Radix::Hex => format!("{sign}0x{n:X}"),
        Radix::Binary => format!("{sign}0b{n:b}"),
        Radix::Octal => format!("{sign}0o{n:o}"),
    })
}

fn trim_zeros(number: &str) -> String {
    if number.contains('.') {
        number
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    } else {
        number.to_owned()
    }
}

fn grouped(number: &str) -> String {
    let (sign, rest) = number.strip_prefix('-').map_or(("", number), |r| ("-", r));
    let (integer, fraction) = rest
        .split_once('.')
        .map_or((rest, None), |(i, f)| (i, Some(f)));
    let mut out = String::with_capacity(number.len() + integer.len() / 3);
    out.push_str(sign);
    for (i, c) in integer.chars().enumerate() {
        if i > 0 && (integer.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(fraction);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn significant_digits() {
        assert_eq!(significant(1234567.891, 10, true), "1,234,567.891");
        assert_eq!(significant(1234567.891, 10, false), "1234567.891");
        assert_eq!(significant(1.0 / 3.0, 10, true), "0.3333333333");
        assert_eq!(significant(3.106_855_961, 6, true), "3.10686");
        assert_eq!(significant(20.0, 10, true), "20");
        assert_eq!(significant(-0.000_000_1, 10, true), "-1e-7");
        assert_eq!(significant(6.02e23, 10, true), "6.02e23");
        assert_eq!(significant(1e9, 6, true), "1,000,000,000");
        assert_eq!(significant(-1234.5, 10, true), "-1,234.5");
    }

    #[test]
    fn money() {
        assert_eq!(fixed(1234.567, 2, true), "1,234.57");
        assert_eq!(fixed(-0.001, 2, true), "0.00");
    }

    #[test]
    fn radixes() {
        assert_eq!(radix(255.0, Radix::Hex).unwrap(), "0xFF");
        assert_eq!(radix(10.0, Radix::Binary).unwrap(), "0b1010");
        assert_eq!(radix(-8.0, Radix::Octal).unwrap(), "-0o10");
        assert!(radix(1.5, Radix::Hex).is_none());
    }
}
