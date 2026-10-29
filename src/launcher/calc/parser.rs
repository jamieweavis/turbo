//! Recursive-descent parser: tokens → [`Query`].
//!
//! ```text
//! query   := expr [ ("in" | "to" | "as" | "into" | "=" | "->") target ]
//! expr    := term { ("+" | "-") term }
//! term    := unary { ("*" | "/" | "x" | "mod" | "%" | "of" | <implicit>) unary }
//! unary   := "-" unary | power
//! power   := postfix [ "^" unary ]
//! postfix := primary { "%" | "!" }
//! primary := number [unit { number unit }] | currency number | "(" expr ")"
//!          | constant | function ( "(" args ")" | power )
//! ```
//!
//! `in` is both a conversion and inches: after a number it means inches when what follows
//! couldn't start a conversion target (`5 in in cm`, `5 in + 2 cm`), otherwise a conversion.

use super::currency::{self, Rates};
use super::functions::{self, Func};
use super::lexer::Tok;
use super::units::{self, Dim, Unit};

/// Deep enough for any real expression, shallow enough that `((((…` can't overflow the stack.
const MAX_DEPTH: usize = 64;

const CONVERSION_WORDS: &[&str] = &["in", "to", "as", "into"];

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Expr {
    Num {
        value: f64,
        text: String,
        radix: bool,
    },
    Const {
        name: &'static str,
        value: f64,
    },
    /// `5 km`, `3 ft 4 in`, `£20`: amounts summed in their dimension.
    Quantity(Vec<Amount>),
    Neg(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    /// `20%`: the value is divided by 100 and remembered as a percentage, so `150 + 10%` adds
    /// ten percent of 150.
    Percent(Box<Expr>),
    /// `20% of 150`.
    PercentOf(Box<Expr>, Box<Expr>),
    Factorial(Box<Expr>),
    Call(Func, Vec<Expr>),
    Paren(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Amount {
    pub value: f64,
    pub text: String,
    pub unit: Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Op {
    Add,
    Sub,
    Mul,
    /// Juxtaposition (`2π`, `2(3+4)`), shown without an operator.
    ImplicitMul,
    Div,
    Mod,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Radix {
    Decimal,
    Hex,
    Binary,
    Octal,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Target {
    Unit(Unit),
    Radix(Radix),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Query {
    pub expr: Expr,
    pub target: Option<Target>,
    /// The conversion was written with `in`, so `5 in cm` can fall back to inches.
    pub via_in: bool,
}

pub(super) fn parse(tokens: &[Tok], rates: Option<&Rates>) -> Option<Query> {
    let mut parser = Parser {
        tokens,
        pos: 0,
        depth: 0,
        rates,
    };
    let expr = parser.expr()?;
    if parser.at_end() {
        return Some(Query {
            expr,
            target: None,
            via_in: false,
        });
    }
    let via_in = matches!(parser.peek(), Some(Tok::Ident(w)) if w == "in");
    if !parser.is_conversion(parser.pos) {
        return None;
    }
    parser.pos += 1;
    let target = parser.target()?;
    parser.at_end().then_some(Query {
        expr,
        target: Some(target),
        via_in,
    })
}

struct Parser<'a> {
    tokens: &'a [Tok],
    pos: usize,
    depth: usize,
    rates: Option<&'a Rates>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn at(&self, pos: usize) -> Option<&Tok> {
        self.tokens.get(pos)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn eat(&mut self, token: &Tok) -> bool {
        if self.peek() == Some(token) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn ident_at(&self, pos: usize) -> Option<&str> {
        match self.at(pos) {
            Some(Tok::Ident(name)) => Some(name),
            _ => None,
        }
    }

    fn is_conversion(&self, pos: usize) -> bool {
        match self.at(pos) {
            Some(Tok::Arrow) => true,
            Some(Tok::Ident(w)) => CONVERSION_WORDS.contains(&w.as_str()),
            _ => false,
        }
    }

    /// Runs `f` one level deeper, failing past `MAX_DEPTH`.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }

    fn expr(&mut self) -> Option<Expr> {
        self.nested(|p| {
            let mut left = p.term()?;
            loop {
                let op = match p.peek() {
                    Some(Tok::Plus) => Op::Add,
                    Some(Tok::Minus) => Op::Sub,
                    _ => return Some(left),
                };
                p.pos += 1;
                let right = p.term()?;
                left = Expr::Bin(op, Box::new(left), Box::new(right));
            }
        })
    }

    fn term(&mut self) -> Option<Expr> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Times) => Op::Mul,
                Some(Tok::Divide) => Op::Div,
                // Left in place by `postfix` because an operand follows.
                Some(Tok::Percent) => Op::Mod,
                Some(Tok::Ident(w)) if w == "mod" => Op::Mod,
                Some(Tok::Ident(w)) if w.eq_ignore_ascii_case("x") => Op::Mul,
                Some(Tok::Ident(w)) if w == "of" && matches!(left, Expr::Percent(_)) => {
                    self.pos += 1;
                    let right = self.unary()?;
                    left = Expr::PercentOf(Box::new(left), Box::new(right));
                    continue;
                }
                Some(Tok::LParen) => {
                    let right = self.power()?;
                    left = Expr::Bin(Op::ImplicitMul, Box::new(left), Box::new(right));
                    continue;
                }
                Some(Tok::Ident(w))
                    if functions::constant(w).is_some() || functions::is_function(w) =>
                {
                    let right = self.power()?;
                    left = Expr::Bin(Op::ImplicitMul, Box::new(left), Box::new(right));
                    continue;
                }
                _ => return Some(left),
            };
            self.pos += 1;
            let right = self.unary()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
    }

    fn unary(&mut self) -> Option<Expr> {
        self.nested(|p| match p.peek() {
            Some(Tok::Minus) => {
                // `-40 c`: the sign belongs to the amount, not to a temperature (which can't
                // be negated as a quantity).
                if matches!(p.at(p.pos + 1), Some(Tok::Num { .. }))
                    && p.unit_at(p.pos + 2).is_some()
                {
                    p.pos += 1;
                    return p.number(true);
                }
                p.pos += 1;
                Some(Expr::Neg(Box::new(p.unary()?)))
            }
            Some(Tok::Plus) => {
                p.pos += 1;
                p.unary()
            }
            _ => p.power(),
        })
    }

    fn power(&mut self) -> Option<Expr> {
        let base = self.postfix()?;
        if self.eat(&Tok::Pow) {
            let exponent = self.unary()?;
            return Some(Expr::Bin(Op::Pow, Box::new(base), Box::new(exponent)));
        }
        Some(base)
    }

    fn postfix(&mut self) -> Option<Expr> {
        let mut expr = self.primary()?;
        loop {
            match self.peek() {
                Some(Tok::Percent) => {
                    // `10 % 3` is modulo: leave it for `term`.
                    let operand_follows = match self.at(self.pos + 1) {
                        Some(Tok::Num { .. } | Tok::LParen) => true,
                        Some(Tok::Ident(w)) => {
                            functions::constant(w).is_some() || functions::is_function(w)
                        }
                        _ => false,
                    };
                    if operand_follows {
                        return Some(expr);
                    }
                    self.pos += 1;
                    expr = Expr::Percent(Box::new(expr));
                }
                Some(Tok::Bang) => {
                    self.pos += 1;
                    expr = Expr::Factorial(Box::new(expr));
                }
                _ => return Some(expr),
            }
        }
    }

    fn primary(&mut self) -> Option<Expr> {
        match self.peek()?.clone() {
            Tok::Num { .. } => self.number(false),
            Tok::Currency(symbol) => {
                self.pos += 1;
                let Some(Tok::Num { value, text, .. }) = self.peek().cloned() else {
                    return None;
                };
                self.pos += 1;
                let unit = currency::unit(currency::code_for_symbol(symbol)?, self.rates);
                Some(Expr::Quantity(vec![Amount { value, text, unit }]))
            }
            Tok::LParen => {
                self.pos += 1;
                let inner = self.expr()?;
                self.eat(&Tok::RParen).then(|| Expr::Paren(Box::new(inner)))
            }
            Tok::Ident(name) => {
                if let Some((shown, value)) = functions::constant(&name) {
                    self.pos += 1;
                    return Some(Expr::Const { name: shown, value });
                }
                let func = functions::function(&name)?;
                self.pos += 1;
                let args = if self.eat(&Tok::LParen) {
                    let mut args = vec![self.expr()?];
                    while self.eat(&Tok::Comma) {
                        args.push(self.expr()?);
                    }
                    if !self.eat(&Tok::RParen) {
                        return None;
                    }
                    args
                } else {
                    // `sqrt 16`, `sin pi`.
                    vec![self.nested(|p| p.power())?]
                };
                Some(Expr::Call(func, args))
            }
            _ => None,
        }
    }

    /// A number, with its unit and any further amounts in the same dimension (`3 ft 4 in`).
    fn number(&mut self, negative: bool) -> Option<Expr> {
        let Some(Tok::Num { value, text, radix }) = self.peek().cloned() else {
            return None;
        };
        self.pos += 1;
        let Some((unit, used)) = self.unit_at(self.pos) else {
            let number = Expr::Num { value, text, radix };
            return Some(if negative {
                Expr::Neg(Box::new(number))
            } else {
                number
            });
        };
        let (value, text) = if negative {
            (-value, format!("−{text}"))
        } else {
            (value, text)
        };
        self.pos += used;
        let mut amounts = vec![Amount { value, text, unit }];

        let combinable = !matches!(amounts[0].unit.dim, Dim::Temperature | Dim::Currency);
        while combinable && let Some(Tok::Num { value, text, .. }) = self.peek().cloned() {
            match self.unit_at(self.pos + 1) {
                Some((unit, used)) if unit.dim == amounts[0].unit.dim => {
                    self.pos += 1 + used;
                    amounts.push(Amount { value, text, unit });
                }
                _ => break,
            }
        }
        Some(Expr::Quantity(amounts))
    }

    /// The unit starting at `pos` and how many tokens it spans, without consuming it.
    fn unit_at(&self, pos: usize) -> Option<(Unit, usize)> {
        match self.at(pos)? {
            Tok::Currency(symbol) => Some((
                currency::unit(currency::code_for_symbol(*symbol)?, self.rates),
                1,
            )),
            Tok::Ident(name) if name == "in" => {
                let inches = match self.at(pos + 1) {
                    None => true,
                    Some(
                        Tok::Plus
                        | Tok::Minus
                        | Tok::Times
                        | Tok::Divide
                        | Tok::Pow
                        | Tok::RParen
                        | Tok::Arrow
                        | Tok::Num { .. },
                    ) => true,
                    Some(Tok::Ident(w)) => CONVERSION_WORDS.contains(&w.as_str()),
                    Some(_) => false,
                };
                inches.then(|| (units::lookup("in").expect("inches"), 1))
            }
            Tok::Ident(name) => self.named_unit(name, pos),
            _ => None,
        }
    }

    fn named_unit(&self, name: &str, pos: usize) -> Option<(Unit, usize)> {
        let next = self.ident_at(pos + 1);
        if name.eq_ignore_ascii_case("fl") && next.is_some_and(|n| n.eq_ignore_ascii_case("oz")) {
            return Some((units::lookup("floz")?, 2));
        }
        if matches!(name, "sq" | "square")
            && let Some(length) = next.and_then(units::lookup)
        {
            return Some((units::square(&length)?, 2));
        }
        if let Some(unit) = units::lookup(name) {
            return Some((unit, 1));
        }
        let code = currency::code_for_word(name)?;
        Some((currency::unit(code, self.rates), 1))
    }

    fn target(&mut self) -> Option<Target> {
        if let Some(name) = self.ident_at(self.pos) {
            let radix = match name.to_ascii_lowercase().as_str() {
                "hex" | "hexadecimal" => Some(Radix::Hex),
                "bin" | "binary" => Some(Radix::Binary),
                "oct" | "octal" => Some(Radix::Octal),
                "dec" | "decimal" => Some(Radix::Decimal),
                _ => None,
            };
            if let Some(radix) = radix {
                self.pos += 1;
                return Some(Target::Radix(radix));
            }
        }
        let (unit, used) = match self.peek()? {
            // `in` as a target is always inches.
            Tok::Ident(name) if name == "in" => (units::lookup("in")?, 1),
            Tok::Ident(name) => self.named_unit(&name.clone(), self.pos)?,
            _ => self.unit_at(self.pos)?,
        };
        self.pos += used;
        Some(Target::Unit(unit))
    }
}

#[cfg(test)]
mod tests {
    use super::super::lexer::lex;
    use super::*;

    fn parse_str(input: &str) -> Option<Query> {
        parse(&lex(input)?, None)
    }

    #[test]
    fn precedence() {
        let query = parse_str("1 + 2 * 3 ^ 2").unwrap();
        let Expr::Bin(Op::Add, _, right) = query.expr else {
            panic!("expected addition at the top");
        };
        assert!(matches!(*right, Expr::Bin(Op::Mul, _, _)));
    }

    #[test]
    fn inches_versus_in() {
        let q = parse_str("5 in in cm").unwrap();
        assert!(matches!(q.expr, Expr::Quantity(ref a) if a[0].unit.symbol == "in"));
        assert!(matches!(q.target, Some(Target::Unit(ref u)) if u.symbol == "cm"));

        let q = parse_str("5 km in miles").unwrap();
        assert!(matches!(q.expr, Expr::Quantity(ref a) if a[0].unit.symbol == "km"));
        assert!(q.via_in);

        let q = parse_str("3 ft 4 in in cm").unwrap();
        assert!(matches!(q.expr, Expr::Quantity(ref a) if a.len() == 2));
    }

    #[test]
    fn percent_versus_modulo() {
        assert!(matches!(
            parse_str("10 % 3").unwrap().expr,
            Expr::Bin(Op::Mod, _, _)
        ));
        assert!(matches!(
            parse_str("20% of 150").unwrap().expr,
            Expr::PercentOf(_, _)
        ));
    }

    #[test]
    fn rejects_app_names_and_junk() {
        for input in [
            "1password",
            "7zip",
            "Excel",
            "x",
            "2 x",
            "sqrt",
            "(1",
            "1 +",
            "5 km in",
        ] {
            assert!(parse_str(input).is_none(), "{input} should not parse");
        }
    }

    #[test]
    fn deep_nesting_fails_instead_of_overflowing() {
        let input = "(".repeat(10_000) + "1" + &")".repeat(10_000);
        assert!(parse_str(&input).is_none());
    }
}
