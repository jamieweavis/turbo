//! The calculator: arithmetic, unit and currency conversion, shown as the palette's top result
//! when the whole query is something worth answering.
//!
//! `lexer` → `parser` → `eval`, all pure and from scratch (no evaluator crate, so units can be
//! part of the grammar). `currency` keeps exchange rates cached and fresh in the background.

mod currency;
mod eval;
mod format;
mod functions;
mod lexer;
mod parser;
mod units;

use std::path::Path;
use std::sync::Arc;

use super::{ResultKind, SearchResult};
use currency::{RateStore, Rates};
use eval::Value;
use parser::{Expr, Op, Query, Radix, Target};
use units::{Dim, Unit};

/// Significant digits for plain arithmetic, and for unit conversions (whose inputs are rarely
/// that precise).
const NUMBER_DIGITS: usize = 10;
const UNIT_DIGITS: usize = 6;

pub(crate) struct Calculator {
    rates: Arc<RateStore>,
    /// Where currency amounts convert to when no target is given.
    local_currency: Option<String>,
}

/// A successful evaluation, shown as the palette's top result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Calculation {
    /// Formatted for reading: `1,234.5`, `3.11 mi`.
    pub display: String,
    /// What ⏎ copies: `1234.5`, `3.11`.
    pub copy_text: String,
    /// The normalised input, shown as the subtitle: `5 km in miles`.
    pub description: String,
}

impl Calculation {
    pub fn result(&self) -> SearchResult {
        SearchResult {
            id: format!("calc:{}", self.copy_text),
            title: self.display.clone(),
            subtitle: self.description.clone(),
            kind: ResultKind::Calculator,
            path: None,
            symbol: Some("equal.circle.fill".into()),
            needs_confirmation: false,
            completion: None,
        }
    }
}

impl Calculator {
    /// Starts loading cached currency rates (and refreshing stale ones) in the background;
    /// `data_dir` holds the cache. Returns immediately.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            rates: RateStore::start(data_dir),
            local_currency: currency::local_currency(),
        }
    }

    /// `None` unless the whole query is an expression worth answering (a bare number or a
    /// word isn't). Pure and fast: called on every keystroke, never touches the network.
    pub fn evaluate(&self, query: &str) -> Option<Calculation> {
        let rates = self.rates.current();
        evaluate_with(query, rates.as_deref(), self.local_currency.as_deref())
    }
}

fn evaluate_with(query: &str, rates: Option<&Rates>, local: Option<&str>) -> Option<Calculation> {
    let tokens = lexer::lex(query)?;
    let query = parser::parse(&tokens, rates)?;
    if !interesting(&query) {
        return None;
    }
    let mut value = eval::eval(&query.expr)?;
    if !value.base.is_finite() {
        return None;
    }

    let mut description = render(&query.expr);
    let target = match &query.target {
        Some(target) => Some(target.clone()),
        // A currency amount on its own converts to the local currency.
        None if value.unit.as_ref().is_some_and(|u| u.dim == Dim::Currency) => {
            let from = value.unit.as_ref()?.symbol.clone();
            let mut to = local.unwrap_or("USD").to_owned();
            if to == from {
                to = if from == "USD" { "EUR" } else { "USD" }.to_owned();
            }
            Some(Target::Unit(currency::unit(&to, rates)))
        }
        None => None,
    };

    // `5 in cm`: with no unit on the number, `in` must have meant inches.
    if query.via_in
        && value.unit.is_none()
        && let Some(Target::Unit(unit)) = &target
        && unit.dim == Dim::Length
    {
        let inches = units::lookup("in")?;
        value.base = inches.to_base(value.base);
        value.unit = Some(inches);
        description.push_str(" in");
    }

    let (display, copy_text) = match &target {
        Some(Target::Radix(radix)) => {
            if value.unit.is_some() {
                return None;
            }
            let text = match radix {
                Radix::Decimal => format::significant(value.base, NUMBER_DIGITS, false),
                _ => format::radix(value.base, *radix)?,
            };
            description.push_str(&format!(" → {}", radix_name(*radix)));
            (text.clone(), text)
        }
        Some(Target::Unit(unit)) => {
            if value.unit.as_ref().map(|u| u.dim) != Some(unit.dim) {
                return None;
            }
            description.push_str(&format!(" → {}", unit.symbol));
            quantity(unit.value_from_base(value.base), unit)?
        }
        None => match &value.unit {
            Some(unit) => quantity(unit.value_from_base(value.base), unit)?,
            None => plain(&value)?,
        },
    };

    let uses_currency = value.unit.as_ref().is_some_and(|u| u.dim == Dim::Currency);
    if uses_currency {
        description.push_str(&format!(" · Rates from {}", rates?.date));
    }
    Some(Calculation {
        display,
        copy_text,
        description,
    })
}

/// Whether showing a result adds anything: a bare number, a lone `e` or a percentage on its
/// own just echoes the query, and is more likely the start of an app's name.
fn interesting(query: &Query) -> bool {
    query.target.is_some() || interesting_expr(&query.expr)
}

fn interesting_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Num { radix, .. } => *radix,
        Expr::Const { name, .. } => *name != "e",
        Expr::Quantity(amounts) => amounts.first().is_some_and(|a| a.unit.dim == Dim::Currency),
        Expr::Neg(inner) | Expr::Paren(inner) => interesting_expr(inner),
        Expr::Percent(_) => false,
        Expr::Bin(..) | Expr::PercentOf(..) | Expr::Factorial(_) | Expr::Call(..) => true,
    }
}

fn plain(value: &Value) -> Option<(String, String)> {
    let v = value.base;
    v.is_finite().then(|| {
        (
            format::significant(v, NUMBER_DIGITS, true),
            format::significant(v, NUMBER_DIGITS, false),
        )
    })
}

/// `3.10686 mi`, `37.78°C`, `£16.75` / `16.75 CHF`; the copy text is just the number.
fn quantity(value: f64, unit: &Unit) -> Option<(String, String)> {
    if !value.is_finite() {
        return None;
    }
    if unit.dim == Dim::Currency {
        let amount = format::fixed(value, 2, true);
        let display = match currency::symbol_for_code(&unit.symbol) {
            Some(symbol) if value < 0.0 => format!("-{symbol}{}", amount.trim_start_matches('-')),
            Some(symbol) => format!("{symbol}{amount}"),
            None => format!("{amount} {}", unit.symbol),
        };
        return Some((display, format::fixed(value, 2, false)));
    }
    let number = format::significant(value, UNIT_DIGITS, true);
    Some((
        format!("{number}{}{}", unit_gap(unit), unit.symbol),
        format::significant(value, UNIT_DIGITS, false),
    ))
}

/// Degree symbols attach to the number (`45°`, `20°C`); everything else gets a space.
fn unit_gap(unit: &Unit) -> &'static str {
    if unit.symbol.starts_with('°') {
        ""
    } else {
        " "
    }
}

fn radix_name(radix: Radix) -> &'static str {
    match radix {
        Radix::Decimal => "decimal",
        Radix::Hex => "hex",
        Radix::Binary => "binary",
        Radix::Octal => "octal",
    }
}

/// The expression echoed back tidily: `(2 + 3) × 4`, `2π`, `20% of 150`.
fn render(expr: &Expr) -> String {
    match expr {
        Expr::Num { text, .. } => text.clone(),
        Expr::Const { name, .. } => (*name).to_owned(),
        Expr::Quantity(amounts) => amounts
            .iter()
            .map(|a| match currency::symbol_for_code(&a.unit.symbol) {
                Some(symbol) if a.unit.dim == Dim::Currency => format!("{symbol}{}", a.text),
                _ => format!("{}{}{}", a.text, unit_gap(&a.unit), a.unit.symbol),
            })
            .collect::<Vec<_>>()
            .join(" "),
        Expr::Neg(inner) => format!("−{}", render(inner)),
        Expr::Paren(inner) => format!("({})", render(inner)),
        Expr::Percent(inner) => format!("{}%", render(inner)),
        Expr::PercentOf(percent, of) => format!("{} of {}", render(percent), render(of)),
        Expr::Factorial(inner) => format!("{}!", render(inner)),
        Expr::Call(func, args) => {
            let args: Vec<String> = args.iter().map(render).collect();
            format!("{}({})", func.name(), args.join(", "))
        }
        Expr::Bin(op, left, right) => {
            let (left, right) = (render(left), render(right));
            match op {
                Op::Add => format!("{left} + {right}"),
                Op::Sub => format!("{left} − {right}"),
                Op::Mul => format!("{left} × {right}"),
                Op::ImplicitMul => format!("{left}{right}"),
                Op::Div => format!("{left} ÷ {right}"),
                Op::Mod => format!("{left} mod {right}"),
                Op::Pow => format!("{left}^{right}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(query: &str) -> Option<Calculation> {
        let rates = currency::test_rates();
        evaluate_with(query, Some(&rates), Some("GBP"))
    }

    fn shows(query: &str, display: &str) {
        let result = calc(query).unwrap_or_else(|| panic!("{query} gave nothing"));
        assert_eq!(result.display, display, "{query}");
    }

    #[test]
    fn arithmetic_results() {
        shows("2+2", "4");
        shows("(2+3)*4", "20");
        shows("1/3", "0.3333333333");
        shows("1000000 * 3", "3,000,000");
        shows("2^64", "1.844674407e19");
        shows("20% of 150", "30");
        shows("150 + 10%", "165");
        shows("pi", "3.141592654");
        shows("0xff", "255");
        shows("sqrt(2)", "1.414213562");
    }

    #[test]
    fn copy_text_and_description() {
        let result = calc("1000 * 1.5").unwrap();
        assert_eq!(result.display, "1,500");
        assert_eq!(result.copy_text, "1500");
        assert_eq!(result.description, "1000 × 1.5");
        assert_eq!(calc("(2+3)×4").unwrap().description, "(2 + 3) × 4");
        assert_eq!(calc("2pi").unwrap().description, "2π");
        assert_eq!(calc("1000 * 1.5").unwrap().result().id, "calc:1500");
    }

    #[test]
    fn unit_conversions() {
        let result = calc("5 km in miles").unwrap();
        assert_eq!(result.display, "3.10686 mi");
        assert_eq!(result.copy_text, "3.10686");
        assert_eq!(result.description, "5 km → mi");
        shows("5km to mi", "3.10686 mi");
        shows("10 kg = lb", "22.0462 lb");
        shows("100f in c", "37.7778°C");
        shows("72 °F to C", "22.2222°C");
        shows("-40 c in f", "-40°F");
        shows("3 ft 4 in in cm", "101.6 cm");
        shows("5 in in cm", "12.7 cm");
        shows("5 in cm", "12.7 cm");
        shows("5 km + 300 m in m", "5,300 m");
        shows("5 km + 300 m", "5.3 km");
        shows("1 GB in MB", "1,000 MB");
        shows("1 GiB in MiB", "1,024 MiB");
        shows("100 Mb in MB", "12.5 MB");
        shows("60 mph in km/h", "96.5606 km/h");
        shows("1 acre in sq m", "4,046.86 m²");
        shows("2 cups in ml", "473.176 ml");
        shows("90 deg in rad", "1.5708 rad");
        shows("2 hours in minutes", "120 min");
    }

    #[test]
    fn radix_conversions() {
        shows("255 in hex", "0xFF");
        shows("0xff in binary", "0b11111111");
        shows("0b1010 in decimal", "10");
        assert!(calc("1.5 in hex").is_none());
    }

    #[test]
    fn currencies() {
        let result = calc("100 usd in gbp").unwrap();
        assert_eq!(result.display, "£64.00");
        assert_eq!(result.copy_text, "64.00");
        assert_eq!(result.description, "$100 → GBP · Rates from 2026-10-23");
        shows("£20 to €", "€25.00");
        shows("$5 in yen", "¥640.00");
        shows("10 chf in usd", "$13.16");
        // On its own, a foreign amount converts to the local currency (GBP here).
        shows("20 eur", "£16.00");
        // …and the local currency to dollars.
        shows("£8", "$12.50");
    }

    #[test]
    fn currencies_need_rates() {
        assert!(evaluate_with("100 usd in gbp", None, Some("GBP")).is_none());
        assert!(evaluate_with("20 eur", None, None).is_none());
    }

    #[test]
    fn leaves_app_searches_alone() {
        for query in [
            "42",
            "2048",
            "e",
            "x",
            "1password",
            "7zip",
            "Excel",
            "Safari",
            "5 km",
            "12 in",
            "50%",
            "-5",
            "(5)",
            "3d printer",
            "4K Video",
            "",
            "  ",
            "sin",
            "1 +",
        ] {
            assert!(calc(query).is_none(), "{query:?} should give nothing");
        }
    }

    #[test]
    fn mismatched_conversions_give_nothing() {
        assert!(calc("5 km in kg").is_none());
        assert!(calc("5 in kg").is_none());
        assert!(calc("1/0").is_none());
        assert!(calc("sqrt(-1)").is_none());
    }

    #[test]
    fn never_panics_on_random_input() {
        const ALPHABET: &[char] = &[
            '0', '1', '9', '.', ',', '+', '-', '*', '/', '^', '%', '!', '(', ')', ' ', 'e', 'x',
            'k', 'm', 'i', 'n', 'p', 's', 'q', 'r', 't', '°', '£', '$', '€', '√', '−', '×', '=',
            '>', 'b', 'o', 'f', 'c', 'g', 'B', 'M', '\'', '"', '²', 'π', '/', 'h', 'u',
        ];
        let rates = currency::test_rates();
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let len = (next() % 24) as usize;
            let query: String = (0..len)
                .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
                .collect();
            let _ = evaluate_with(&query, Some(&rates), Some("GBP"));
        }
    }

    #[test]
    fn is_fast() {
        let rates = currency::test_rates();
        let queries = [
            "5 km in miles",
            "(2+3)*4^2/7",
            "100 usd in gbp",
            "sqrt(2) * sin(30°) + 20% of 150",
            "Safari",
        ];
        let start = std::time::Instant::now();
        let rounds = 1000;
        for _ in 0..rounds {
            for query in queries {
                let _ = evaluate_with(query, Some(&rates), Some("GBP"));
            }
        }
        let per_query = start.elapsed() / (rounds * queries.len() as u32);
        // Generous for debug builds; release is a few microseconds.
        assert!(per_query.as_micros() < 500, "{per_query:?} per query");
    }
}
