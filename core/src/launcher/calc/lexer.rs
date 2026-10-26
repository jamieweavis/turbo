//! Splits a query into tokens. Anything it doesn't recognise makes the whole query fail, which
//! is how most app names ("1Password", "7-Zip") are kept out of the calculator.

use super::{currency, functions, units};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Tok {
    Num {
        value: f64,
        /// As typed, for echoing back in the description.
        text: String,
        /// Written in hex, binary or octal.
        radix: bool,
    },
    Ident(String),
    /// A currency symbol: `$`, `£`, `€`, …
    Currency(char),
    Plus,
    Minus,
    Times,
    Divide,
    Pow,
    Percent,
    Bang,
    LParen,
    RParen,
    Comma,
    /// `=`, `->` or `→`: a conversion, like `in`.
    Arrow,
}

pub(super) fn lex(input: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let simple = match c {
            _ if c.is_whitespace() => {
                i += 1;
                continue;
            }
            '+' => Some(Tok::Plus),
            '-' if next == Some('>') => {
                i += 1;
                Some(Tok::Arrow)
            }
            '-' | '−' | '–' => Some(Tok::Minus),
            '*' if next == Some('*') => {
                i += 1;
                Some(Tok::Pow)
            }
            '*' | '×' | '·' | '⋅' => Some(Tok::Times),
            '/' | '÷' => Some(Tok::Divide),
            '^' => Some(Tok::Pow),
            '%' => Some(Tok::Percent),
            '!' => Some(Tok::Bang),
            '(' => Some(Tok::LParen),
            ')' => Some(Tok::RParen),
            ',' => Some(Tok::Comma),
            '=' | '→' => Some(Tok::Arrow),
            '√' => Some(Tok::Ident("sqrt".into())),
            '\'' | '"' => Some(Tok::Ident(c.to_string())),
            _ if currency::is_symbol(c) => Some(Tok::Currency(c)),
            _ => None,
        };
        if let Some(token) = simple {
            tokens.push(token);
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && next.is_some_and(|n| n.is_ascii_digit())) {
            let (token, end) = number(&chars, i)?;
            tokens.push(token);
            i = end;
        } else if c.is_alphabetic() || c == '°' {
            let (name, end) = ident(&chars, i);
            tokens.push(Tok::Ident(name));
            i = end;
        } else {
            return None;
        }
    }
    Some(tokens)
}

fn number(chars: &[char], start: usize) -> Option<(Tok, usize)> {
    let text = |end: usize| chars[start..end].iter().collect::<String>();

    // 0x…, 0b…, 0o…
    if chars[start] == '0'
        && let Some(radix) = chars.get(start + 1).and_then(|c| match c {
            'x' | 'X' => Some(16),
            'b' | 'B' => Some(2),
            'o' | 'O' => Some(8),
            _ => None,
        })
    {
        let mut end = start + 2;
        while chars.get(end).is_some_and(|c| c.is_digit(radix)) {
            end += 1;
        }
        if end > start + 2 {
            let digits: String = chars[start + 2..end].iter().collect();
            let value = u64::from_str_radix(&digits, radix).ok()? as f64;
            let token = Tok::Num {
                value,
                text: text(end),
                radix: true,
            };
            return Some((token, end));
        }
    }

    let digit = |i: usize| chars.get(i).is_some_and(|c| c.is_ascii_digit());
    let mut digits = String::new();
    let mut end = start;
    let mut run = 0;
    while end < chars.len() {
        let c = chars[end];
        if c.is_ascii_digit() {
            digits.push(c);
            run += 1;
            end += 1;
        } else if c == ','
            && (1..=3).contains(&run)
            && (1..=3).all(|k| digit(end + k))
            && !digit(end + 4)
            && !digits.contains('.')
        {
            // A thousands separator: exactly three digits follow. `max(1, 2)` (with a space)
            // is still an argument list.
            run = 0;
            end += 1;
        } else if c == '.' && !digits.contains('.') && digit(end + 1) {
            digits.push('.');
            end += 1;
        } else {
            break;
        }
    }
    // An exponent only when digits follow: `2e` is 2 × e.
    if matches!(chars.get(end), Some('e' | 'E')) {
        let sign = matches!(chars.get(end + 1), Some('+' | '-'));
        let first = end + 1 + usize::from(sign);
        if digit(first) {
            digits.push('e');
            if sign {
                digits.push(chars[end + 1]);
            }
            end = first;
            while digit(end) {
                digits.push(chars[end]);
                end += 1;
            }
        }
    }
    let value: f64 = digits.parse().ok()?;
    let token = Tok::Num {
        value,
        text: text(end),
        radix: false,
    };
    Some((token, end))
}

/// Letters (and `°`, `²`, `³`); digits only when they make a known name (`log2`, `km2`), so
/// `2x3` is 2 × 3. `km/h`-style unit names are joined into one token.
fn ident(chars: &[char], start: usize) -> (String, usize) {
    let mut end = start + 1;
    while chars
        .get(end)
        .is_some_and(|c| c.is_alphabetic() || matches!(c, '²' | '³'))
    {
        end += 1;
    }
    let mut name: String = chars[start..end].iter().collect();

    let mut with_digits = end;
    while chars.get(with_digits).is_some_and(|c| c.is_ascii_digit()) {
        with_digits += 1;
    }
    if with_digits > end {
        let candidate: String = chars[start..with_digits].iter().collect();
        if units::lookup(&candidate).is_some() || functions::is_function(&candidate) {
            name = candidate;
            end = with_digits;
        }
    }

    if chars.get(end) == Some(&'/') {
        let mut rest = end + 1;
        while chars.get(rest).is_some_and(|c| c.is_alphabetic()) {
            rest += 1;
        }
        let candidate: String = chars[start..rest].iter().collect();
        if rest > end + 1 && units::lookup(&candidate).is_some() {
            name = candidate;
            end = rest;
        }
    }
    (name, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(value: f64, text: &str) -> Tok {
        Tok::Num {
            value,
            text: text.into(),
            radix: false,
        }
    }

    fn id(name: &str) -> Tok {
        Tok::Ident(name.into())
    }

    #[test]
    fn numbers() {
        assert_eq!(lex("1,000.5").unwrap(), vec![num(1000.5, "1,000.5")]);
        assert_eq!(lex(".5").unwrap(), vec![num(0.5, ".5")]);
        assert_eq!(lex("1e3").unwrap(), vec![num(1000.0, "1e3")]);
        assert_eq!(lex("2e").unwrap(), vec![num(2.0, "2"), id("e")]);
        assert_eq!(
            lex("0xff").unwrap(),
            vec![Tok::Num {
                value: 255.0,
                text: "0xff".into(),
                radix: true
            }]
        );
        // Not a separator: four digits after the comma, or a space.
        assert_eq!(
            lex("1,2345").unwrap(),
            vec![num(1.0, "1"), Tok::Comma, num(2345.0, "2345")]
        );
        assert_eq!(
            lex("max(1, 234)").unwrap(),
            vec![
                id("max"),
                Tok::LParen,
                num(1.0, "1"),
                Tok::Comma,
                num(234.0, "234"),
                Tok::RParen
            ]
        );
    }

    #[test]
    fn identifiers() {
        assert_eq!(
            lex("2x3").unwrap(),
            vec![num(2.0, "2"), id("x"), num(3.0, "3")]
        );
        assert_eq!(lex("log2(8)").unwrap()[0], id("log2"));
        assert_eq!(lex("5 km2").unwrap()[1], id("km2"));
        assert_eq!(lex("60 km/h").unwrap()[1], id("km/h"));
        assert_eq!(lex("72 °F").unwrap()[1], id("°F"));
        assert_eq!(
            lex("10 m/2").unwrap()[1..],
            [id("m"), Tok::Divide, num(2.0, "2")]
        );
    }

    #[test]
    fn symbols() {
        assert_eq!(
            lex("£20 → €").unwrap(),
            vec![
                Tok::Currency('£'),
                num(20.0, "20"),
                Tok::Arrow,
                Tok::Currency('€')
            ]
        );
        assert_eq!(lex("2 ** 3").unwrap()[1], Tok::Pow);
        assert!(lex("2 & 3").is_none());
    }
}
