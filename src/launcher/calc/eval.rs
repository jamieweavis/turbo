//! Evaluates a parsed expression to a number, optionally with a unit.

use super::parser::{Expr, Op};
use super::units::{Dim, Unit};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Value {
    /// In the dimension's base unit (or a plain number when `unit` is `None`).
    pub base: f64,
    /// The unit to show it in: the first unit written.
    pub unit: Option<Unit>,
    /// Written as a percentage, so `150 + 10%` means 150 × 1.1.
    pub percent: bool,
}

impl Value {
    fn number(value: f64) -> Self {
        Self {
            base: value,
            unit: None,
            percent: false,
        }
    }

    fn with_unit(base: f64, unit: Option<Unit>) -> Self {
        Self {
            base,
            unit,
            percent: false,
        }
    }

    fn dim(&self) -> Option<Dim> {
        self.unit.as_ref().map(|u| u.dim)
    }

    /// Temperatures are points on a scale, not amounts: they can be converted but not added
    /// or scaled.
    fn is_temperature(&self) -> bool {
        self.dim() == Some(Dim::Temperature)
    }
}

/// `None` for anything that doesn't make sense (adding metres to kilograms, `sqrt(5 km)`).
/// Non-finite results are left for the caller to reject.
pub(super) fn eval(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Num { value, .. } | Expr::Const { value, .. } => Some(Value::number(*value)),
        Expr::Quantity(amounts) => {
            let unit = amounts.first()?.unit.clone();
            let base = amounts.iter().map(|a| a.unit.to_base(a.value)).sum();
            Some(Value::with_unit(base, Some(unit)))
        }
        Expr::Neg(inner) => {
            let value = eval(inner)?;
            if value.is_temperature() {
                return None;
            }
            Some(Value {
                base: -value.base,
                ..value
            })
        }
        Expr::Paren(inner) => eval(inner),
        Expr::Percent(inner) => {
            let value = eval(inner)?;
            if value.unit.is_some() {
                return None;
            }
            Some(Value {
                base: value.base / 100.0,
                unit: None,
                percent: true,
            })
        }
        Expr::PercentOf(percent, of) => {
            let (percent, of) = (eval(percent)?, eval(of)?);
            if of.is_temperature() {
                return None;
            }
            Some(Value::with_unit(percent.base * of.base, of.unit))
        }
        Expr::Factorial(inner) => {
            let value = eval(inner)?;
            let n = value.base;
            if value.unit.is_some() || n < 0.0 || n.fract() != 0.0 || n > 170.0 {
                return None;
            }
            Some(Value::number((1..=n as u64).map(|k| k as f64).product()))
        }
        Expr::Call(func, args) => {
            let mut numbers = Vec::with_capacity(args.len());
            for arg in args {
                let value = eval(arg)?;
                match value.dim() {
                    None => numbers.push(value.base),
                    // Angles are already in radians in base units.
                    Some(Dim::Angle) if func.takes_angle() => numbers.push(value.base),
                    Some(_) => return None,
                }
            }
            Some(Value::number(func.apply(&numbers)?))
        }
        Expr::Bin(op, left, right) => binary(*op, eval(left)?, eval(right)?),
    }
}

fn binary(op: Op, left: Value, right: Value) -> Option<Value> {
    if left.is_temperature() || right.is_temperature() {
        return None;
    }
    match op {
        Op::Add | Op::Sub => {
            let sign = if op == Op::Add { 1.0 } else { -1.0 };
            if right.percent && !left.percent {
                // `150 + 10%`
                return Some(Value::with_unit(
                    left.base * (1.0 + sign * right.base),
                    left.unit,
                ));
            }
            if left.dim() != right.dim() {
                return None;
            }
            Some(Value {
                base: left.base + sign * right.base,
                unit: left.unit,
                percent: left.percent && right.percent,
            })
        }
        Op::Mul | Op::ImplicitMul => match (&left.unit, &right.unit) {
            (_, None) => Some(Value::with_unit(left.base * right.base, left.unit)),
            (None, Some(_)) => Some(Value::with_unit(left.base * right.base, right.unit)),
            (Some(_), Some(_)) => None,
        },
        Op::Div => match (left.dim(), right.dim()) {
            (_, None) => Some(Value::with_unit(left.base / right.base, left.unit)),
            // A ratio of like quantities: `1 km / 100 m` is 10.
            (Some(a), Some(b)) if a == b => Some(Value::number(left.base / right.base)),
            _ => None,
        },
        Op::Mod => (left.unit.is_none() && right.unit.is_none())
            .then(|| Value::number(left.base % right.base)),
        Op::Pow => (left.unit.is_none() && right.unit.is_none())
            .then(|| Value::number(left.base.powf(right.base))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::lexer::lex;
    use super::super::parser::parse;
    use super::*;

    fn value(input: &str) -> Option<f64> {
        let query = parse(&lex(input)?, None)?;
        eval(&query.expr).map(|v| v.base)
    }

    fn close(input: &str, expected: f64) {
        let actual = value(input).unwrap_or_else(|| panic!("{input} didn't evaluate"));
        assert!(
            (actual - expected).abs() <= 1e-9 * expected.abs().max(1.0),
            "{input} = {actual}, expected {expected}"
        );
    }

    #[test]
    fn arithmetic() {
        close("1 + 2 * 3", 7.0);
        close("(2+3)×4", 20.0);
        close("2^3^2", 512.0);
        close("-2^2", -4.0);
        close("2 ** 10", 1024.0);
        close("10 ÷ 4", 2.5);
        close("7 x 6", 42.0);
        close("10 % 3", 1.0);
        close("10 mod 4", 2.0);
        close("5!", 120.0);
        close("1,000.5 * 2", 2001.0);
    }

    #[test]
    fn implicit_multiplication_constants_and_functions() {
        close("2(3+4)", 14.0);
        close("2pi", std::f64::consts::TAU);
        close("sqrt 16", 4.0);
        close("sqrt(2)^2", 2.0);
        close("sin(30°)", 0.5);
        close("log 1000", 3.0);
        close("log2(8)", 3.0);
        close("max(3, 9, 4)", 9.0);
        close("√9", 3.0);
    }

    #[test]
    fn percentages() {
        close("20% of 150", 30.0);
        close("150 + 10%", 165.0);
        close("150 - 10%", 135.0);
        close("200 * 10%", 20.0);
    }

    #[test]
    fn units_combine_sensibly() {
        close("5 km + 300 m", 5300.0);
        close("3 ft 4 in", 1.016);
        close("1 km / 100 m", 10.0);
        assert!(value("5 km + 3 kg").is_none());
        assert!(value("5 km * 3 m").is_none());
        assert!(value("100 c + 5").is_none());
        assert!(value("sqrt(5 km)").is_none());
    }
}
