//! Preserve Python 3.14's string syntax for the exporter's numeric settings.
//!
//! Unicode decimal digits and underscores follow `float()` / base-ten `int()`:
//! <https://docs.python.org/3.14/library/functions.html#float>
//! <https://docs.python.org/3.14/library/functions.html#int>

/// Parse the legacy positive, finite seconds setting without changing its range.
pub(crate) fn positive_float(value: &str) -> Option<f64> {
    let number: f64 = normalize(value)?.parse().ok()?;
    (number.is_finite() && number > 0.0).then_some(number)
}

/// Parse the legacy base-ten integer setting, then enforce the valid port range.
pub(crate) fn port(value: &str) -> Option<u16> {
    let normalized = normalize(value)?;
    let digits = normalized.strip_prefix('+').unwrap_or(&normalized);
    // Python 3.14's default integer-string limit also counts leading zeroes.
    if digits.is_empty() || digits.len() > 4300 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port: u16 = digits.parse().ok()?;
    (port > 0).then_some(port)
}

fn normalize(value: &str) -> Option<String> {
    let mut chars = value.trim_matches(number_whitespace).chars().peekable();
    let mut output = String::with_capacity(value.len());
    let mut previous_digit = false;
    while let Some(ch) = chars.next() {
        if let Some(digit) = decimal_digit(ch) {
            output.push(char::from(b'0' + digit));
            previous_digit = true;
        } else if ch == '_' {
            // Removing arbitrary underscores would accept malformed Python input.
            if !previous_digit || chars.peek().copied().and_then(decimal_digit).is_none() {
                return None;
            }
            previous_digit = false;
        } else if ch.is_ascii() {
            output.push(ch);
            previous_digit = false;
        } else {
            return None;
        }
    }
    Some(output)
}

fn number_whitespace(ch: char) -> bool {
    // Unlike str.strip(), float()/int() reject ASCII separators U+001C..U+001F.
    matches!(
        ch,
        '\u{0009}'..='\u{000d}'
            | '\u{0020}'
            | '\u{0085}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

fn decimal_digit(ch: char) -> Option<u8> {
    let codepoint = u32::from(ch);
    let index = DECIMAL_ZEROES
        .partition_point(|&zero| zero <= codepoint)
        .checked_sub(1)?;
    let digit = codepoint - DECIMAL_ZEROES[index];
    (digit < 10).then_some(digit as u8)
}

// Generated from CPython 3.14.8's unicodedata, Unicode Character Database 16.0.0.
// Every Nd block comprises ten consecutive characters with decimal values 0..9.
// Reproduce with:
// [i for i in range(0x110000)
//  if unicodedata.category(chr(i)) == "Nd" and unicodedata.decimal(chr(i)) == 0]
const DECIMAL_ZEROES: &[u32] = &[
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
    0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
    0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10,
    0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0,
    0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50, 0x11d50,
    0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce, 0x1d7d8,
    0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_python_float_digits_grouping_and_grammar() {
        for (input, expected) in [
            ("1_000", 1000.0),
            ("+000_001", 1.0),
            ("١٢.٣e+٤", 123000.0),
            ("９_１７３", 9173.0),
            ("1_2.3_4E+0_2", 1234.0),
            ("+.5", 0.5),
            ("1.", 1.0),
            ("1.e2", 100.0),
            ("１.２", 1.2),
            ("1_٢.३_４e-５", 0.0001234),
        ] {
            assert_eq!(positive_float(input), Some(expected), "{input:?}");
        }
    }

    #[test]
    fn accepts_python_base_ten_port_digits_grouping_and_leading_zeroes() {
        for (input, expected) in [
            ("1_000", 1000),
            ("９_１７３", 9173),
            ("+000_001", 1),
            ("٠٠٩١٧٣", 9173),
            ("6_5_5_3_5", 65535),
            ("1_٢३４", 1234),
        ] {
            assert_eq!(port(input), Some(expected), "{input:?}");
        }
        assert_eq!(port("1"), Some(1));
        assert_eq!(port("65535"), Some(65535));
    }

    #[test]
    fn accepts_every_python_number_whitespace_at_boundaries_only() {
        let whitespace = [
            '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{85}', '\u{a0}', '\u{1680}', '\u{2000}',
            '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}',
            '\u{2008}', '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}',
            '\u{3000}',
        ];
        for ch in whitespace {
            assert_eq!(positive_float(&format!("{ch}1_000{ch}")), Some(1000.0));
            assert_eq!(port(&format!("{ch}９_１７３{ch}")), Some(9173));
            assert_eq!(positive_float(&format!("1{ch}2")), None);
            assert_eq!(port(&format!("1{ch}2")), None);
        }
    }

    #[test]
    fn rejects_strip_only_controls_and_other_non_numeric_unicode() {
        for ch in [
            '\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}', '\u{180e}', '\u{200b}', '\u{feff}',
        ] {
            assert_eq!(positive_float(&format!("{ch}1{ch}")), None);
            assert_eq!(port(&format!("{ch}1{ch}")), None);
        }
        for value in ["²", "①", "Ⅻ", "¼", "−1", "＋1", "１．２", "１ｅ２"] {
            assert_eq!(positive_float(value), None, "{value:?}");
            assert_eq!(port(value), None, "{value:?}");
        }
    }

    #[test]
    fn rejects_malformed_underscores_and_non_decimal_forms() {
        for value in [
            "", " ", "_1", "1_", "1__0", "+_1", "1_.0", "1._0", "1_e2", "1e_2", "1e+_2", "1e_+2",
            "._1", "0x10", "0x_10", "0b10", "0o10", "1\0", "1 2", "++1", "1f", ".", "e1", "1e",
            "1e+", "1,5",
        ] {
            assert_eq!(positive_float(value), None, "{value:?}");
            assert_eq!(port(value), None, "{value:?}");
        }
    }

    #[test]
    fn keeps_positive_finite_float_range_including_subnormals() {
        for value in [
            "0", "-0", "-1", "nan", "+NaN", "inf", "Infinity", "1e309", "1e-9999",
        ] {
            assert_eq!(positive_float(value), None, "{value:?}");
        }
        assert_eq!(positive_float("1e-300"), Some(1e-300));
        assert_eq!(positive_float("1e300"), Some(1e300));
        assert_eq!(positive_float("5e-324"), Some(f64::from_bits(1)));
        assert_eq!(positive_float("1.7976931348623157e308"), Some(f64::MAX));
    }

    #[test]
    fn enforces_integer_port_range_and_python_default_digit_limit() {
        for value in [
            "0",
            "-0",
            "-1",
            "65536",
            "99999999999999999999",
            "1.0",
            "1e2",
            "nan",
            "inf",
        ] {
            assert_eq!(port(value), None, "{value:?}");
        }
        assert_eq!(port(&format!("{}1", "0".repeat(4299))), Some(1));
        assert_eq!(port(&format!("{}1", "0".repeat(4300))), None);
    }

    #[test]
    fn every_unicode_16_decimal_digit_block_is_supported() {
        for &zero in DECIMAL_ZEROES {
            for digit in 0..10 {
                let ch = char::from_u32(zero + digit).unwrap();
                assert_eq!(decimal_digit(ch), Some(digit as u8));
                assert_eq!(port(&format!("1{ch}")), Some(10 + digit as u16));
                assert_eq!(
                    positive_float(&format!("1.{ch}")),
                    Some(1.0 + f64::from(digit) / 10.0)
                );
            }
        }
    }
}
