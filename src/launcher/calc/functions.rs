//! Named functions and constants.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Func {
    Sqrt,
    Cbrt,
    Abs,
    Round,
    Floor,
    Ceil,
    Ln,
    Log10,
    Log2,
    Exp,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Min,
    Max,
}

const FUNCS: &[(&str, Func)] = &[
    ("sqrt", Func::Sqrt),
    ("cbrt", Func::Cbrt),
    ("abs", Func::Abs),
    ("round", Func::Round),
    ("floor", Func::Floor),
    ("ceil", Func::Ceil),
    ("ln", Func::Ln),
    ("log", Func::Log10),
    ("log10", Func::Log10),
    ("log2", Func::Log2),
    ("exp", Func::Exp),
    ("sin", Func::Sin),
    ("cos", Func::Cos),
    ("tan", Func::Tan),
    ("asin", Func::Asin),
    ("acos", Func::Acos),
    ("atan", Func::Atan),
    ("min", Func::Min),
    ("max", Func::Max),
];

const CONSTANTS: &[(&str, &str, f64)] = &[
    ("pi", "π", std::f64::consts::PI),
    ("π", "π", std::f64::consts::PI),
    ("tau", "τ", std::f64::consts::TAU),
    ("τ", "τ", std::f64::consts::TAU),
    ("e", "e", std::f64::consts::E),
];

pub(super) fn function(name: &str) -> Option<Func> {
    let lower = name.to_ascii_lowercase();
    FUNCS.iter().find(|(n, _)| *n == lower).map(|(_, f)| *f)
}

pub(super) fn is_function(name: &str) -> bool {
    function(name).is_some()
}

/// `(display name, value)`.
pub(super) fn constant(name: &str) -> Option<(&'static str, f64)> {
    CONSTANTS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, shown, value)| (*shown, *value))
}

impl Func {
    pub fn name(self) -> &'static str {
        FUNCS
            .iter()
            .find(|(_, f)| *f == self)
            .map_or("?", |(n, _)| n)
    }

    pub fn is_variadic(self) -> bool {
        matches!(self, Func::Min | Func::Max)
    }

    /// Whether the argument is an angle (radians, or any angle unit converted to them).
    pub fn takes_angle(self) -> bool {
        matches!(self, Func::Sin | Func::Cos | Func::Tan)
    }

    pub fn apply(self, args: &[f64]) -> Option<f64> {
        let x = *args.first()?;
        if !self.is_variadic() && args.len() != 1 {
            return None;
        }
        Some(match self {
            Func::Sqrt => x.sqrt(),
            Func::Cbrt => x.cbrt(),
            Func::Abs => x.abs(),
            Func::Round => x.round(),
            Func::Floor => x.floor(),
            Func::Ceil => x.ceil(),
            Func::Ln => x.ln(),
            Func::Log10 => x.log10(),
            Func::Log2 => x.log2(),
            Func::Exp => x.exp(),
            Func::Sin => x.sin(),
            Func::Cos => x.cos(),
            Func::Tan => x.tan(),
            Func::Asin => x.asin(),
            Func::Acos => x.acos(),
            Func::Atan => x.atan(),
            Func::Min => args.iter().copied().fold(f64::INFINITY, f64::min),
            Func::Max => args.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        })
    }
}
