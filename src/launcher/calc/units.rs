//! Physical units: what each name means and how to convert it.
//!
//! Every unit converts to its dimension's base unit (metre, kilogram, litre, kelvin, second,
//! byte, metre per second, square metre, radian) as `base = value × factor + offset`; only
//! temperatures have an offset.
//!
//! Names match case-sensitively first, so `MB` (megabyte) and `Mb` (megabit) differ. Failing
//! that they match ignoring case, preferring the earlier definition, so casual `mb` and `kb`
//! mean bytes and `c` / `f` mean Celsius / Fahrenheit.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dim {
    Length,
    Mass,
    Volume,
    Temperature,
    Time,
    Data,
    Speed,
    Area,
    Angle,
    Currency,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Unit {
    /// How results show it: `km`, `°F`, or an ISO code for currencies.
    pub symbol: String,
    pub dim: Dim,
    pub factor: f64,
    pub offset: f64,
}

impl Unit {
    pub fn to_base(&self, value: f64) -> f64 {
        value * self.factor + self.offset
    }

    pub fn value_from_base(&self, base: f64) -> f64 {
        (base - self.offset) / self.factor
    }
}

struct Def {
    symbol: &'static str,
    names: &'static [&'static str],
    dim: Dim,
    factor: f64,
    offset: f64,
}

const fn def(symbol: &'static str, names: &'static [&'static str], dim: Dim, factor: f64) -> Def {
    Def {
        symbol,
        names,
        dim,
        factor,
        offset: 0.0,
    }
}

const FAHRENHEIT_FACTOR: f64 = 5.0 / 9.0;
const CELSIUS_OFFSET: f64 = 273.15;

/// Order matters for case-insensitive matches: the first definition wins.
const DEFS: &[Def] = &[
    // Length (metre)
    def(
        "nm",
        &["nm", "nanometer", "nanometers", "nanometre", "nanometres"],
        Dim::Length,
        1e-9,
    ),
    def(
        "µm",
        &[
            "µm",
            "um",
            "micrometer",
            "micrometers",
            "micrometre",
            "micrometres",
            "micron",
            "microns",
        ],
        Dim::Length,
        1e-6,
    ),
    def(
        "mm",
        &[
            "mm",
            "millimeter",
            "millimeters",
            "millimetre",
            "millimetres",
        ],
        Dim::Length,
        1e-3,
    ),
    def(
        "cm",
        &[
            "cm",
            "centimeter",
            "centimeters",
            "centimetre",
            "centimetres",
        ],
        Dim::Length,
        1e-2,
    ),
    def(
        "m",
        &["m", "meter", "meters", "metre", "metres"],
        Dim::Length,
        1.0,
    ),
    def(
        "km",
        &[
            "km",
            "kms",
            "kilometer",
            "kilometers",
            "kilometre",
            "kilometres",
        ],
        Dim::Length,
        1e3,
    ),
    def("in", &["in", "inch", "inches", "\""], Dim::Length, 0.0254),
    def("ft", &["ft", "foot", "feet", "'"], Dim::Length, 0.3048),
    def("yd", &["yd", "yds", "yard", "yards"], Dim::Length, 0.9144),
    def("mi", &["mi", "mile", "miles"], Dim::Length, 1609.344),
    def("nmi", &["nmi"], Dim::Length, 1852.0),
    def(
        "ly",
        &["ly", "lightyear", "lightyears"],
        Dim::Length,
        9.460_730_472_580_8e15,
    ),
    // Mass (kilogram)
    def("mg", &["mg", "milligram", "milligrams"], Dim::Mass, 1e-6),
    def(
        "g",
        &["g", "gram", "grams", "gramme", "grammes"],
        Dim::Mass,
        1e-3,
    ),
    def(
        "kg",
        &["kg", "kgs", "kilogram", "kilograms", "kilo", "kilos"],
        Dim::Mass,
        1.0,
    ),
    def("t", &["t", "tonne", "tonnes"], Dim::Mass, 1e3),
    def(
        "lb",
        &["lb", "lbs", "pound", "pounds"],
        Dim::Mass,
        0.453_592_37,
    ),
    def(
        "oz",
        &["oz", "ounce", "ounces"],
        Dim::Mass,
        0.028_349_523_125,
    ),
    def("st", &["st", "stone", "stones"], Dim::Mass, 6.350_293_18),
    def("ton", &["ton", "tons"], Dim::Mass, 907.184_74),
    // Volume (litre); US customary measures
    def(
        "ml",
        &[
            "ml",
            "mL",
            "milliliter",
            "milliliters",
            "millilitre",
            "millilitres",
        ],
        Dim::Volume,
        1e-3,
    ),
    def(
        "cl",
        &[
            "cl",
            "cL",
            "centiliter",
            "centiliters",
            "centilitre",
            "centilitres",
        ],
        Dim::Volume,
        1e-2,
    ),
    def(
        "dl",
        &[
            "dl",
            "dL",
            "deciliter",
            "deciliters",
            "decilitre",
            "decilitres",
        ],
        Dim::Volume,
        1e-1,
    ),
    def(
        "L",
        &["L", "l", "liter", "liters", "litre", "litres"],
        Dim::Volume,
        1.0,
    ),
    def("m³", &["m³", "m3"], Dim::Volume, 1e3),
    def("cm³", &["cm³", "cm3", "cc"], Dim::Volume, 1e-3),
    def(
        "gal",
        &["gal", "gallon", "gallons"],
        Dim::Volume,
        3.785_411_784,
    ),
    def("qt", &["qt", "quart", "quarts"], Dim::Volume, 0.946_352_946),
    def("pt", &["pt", "pint", "pints"], Dim::Volume, 0.473_176_473),
    def("cup", &["cup", "cups"], Dim::Volume, 0.236_588_236_5),
    def("fl oz", &["floz"], Dim::Volume, 0.029_573_529_562_5),
    def(
        "tbsp",
        &["tbsp", "tablespoon", "tablespoons"],
        Dim::Volume,
        0.014_786_764_781_25,
    ),
    def(
        "tsp",
        &["tsp", "teaspoon", "teaspoons"],
        Dim::Volume,
        0.004_928_921_593_75,
    ),
    // Temperature (kelvin)
    Def {
        symbol: "°C",
        names: &["°C", "°c", "C", "c", "celsius", "centigrade", "degC"],
        dim: Dim::Temperature,
        factor: 1.0,
        offset: CELSIUS_OFFSET,
    },
    Def {
        symbol: "°F",
        names: &["°F", "°f", "F", "f", "fahrenheit", "degF"],
        dim: Dim::Temperature,
        factor: FAHRENHEIT_FACTOR,
        offset: CELSIUS_OFFSET - 32.0 * FAHRENHEIT_FACTOR,
    },
    def("K", &["K", "kelvin", "kelvins"], Dim::Temperature, 1.0),
    // Time (second)
    def(
        "ms",
        &["ms", "millisecond", "milliseconds"],
        Dim::Time,
        1e-3,
    ),
    def(
        "s",
        &["s", "sec", "secs", "second", "seconds"],
        Dim::Time,
        1.0,
    ),
    def(
        "min",
        &["min", "mins", "minute", "minutes"],
        Dim::Time,
        60.0,
    ),
    def("h", &["h", "hr", "hrs", "hour", "hours"], Dim::Time, 3600.0),
    def("d", &["d", "day", "days"], Dim::Time, 86_400.0),
    def("wk", &["wk", "wks", "week", "weeks"], Dim::Time, 604_800.0),
    // Average Gregorian month and year.
    def("mo", &["month", "months"], Dim::Time, 2_629_746.0),
    def(
        "yr",
        &["yr", "yrs", "year", "years"],
        Dim::Time,
        31_556_952.0,
    ),
    // Data (byte). kB/MB/GB are decimal, as macOS shows file sizes; KiB/MiB/GiB binary.
    def("B", &["B", "byte", "bytes"], Dim::Data, 1.0),
    def("kB", &["kB", "KB", "kilobyte", "kilobytes"], Dim::Data, 1e3),
    def("MB", &["MB", "megabyte", "megabytes"], Dim::Data, 1e6),
    def("GB", &["GB", "gigabyte", "gigabytes"], Dim::Data, 1e9),
    def("TB", &["TB", "terabyte", "terabytes"], Dim::Data, 1e12),
    def("PB", &["PB", "petabyte", "petabytes"], Dim::Data, 1e15),
    def("KiB", &["KiB", "kibibyte", "kibibytes"], Dim::Data, 1024.0),
    def(
        "MiB",
        &["MiB", "mebibyte", "mebibytes"],
        Dim::Data,
        1_048_576.0,
    ),
    def(
        "GiB",
        &["GiB", "gibibyte", "gibibytes"],
        Dim::Data,
        1_073_741_824.0,
    ),
    def(
        "TiB",
        &["TiB", "tebibyte", "tebibytes"],
        Dim::Data,
        1_099_511_627_776.0,
    ),
    def("bit", &["bit", "bits"], Dim::Data, 0.125),
    def(
        "kbit",
        &["Kb", "kbit", "kilobit", "kilobits"],
        Dim::Data,
        125.0,
    ),
    def(
        "Mbit",
        &["Mb", "Mbit", "megabit", "megabits"],
        Dim::Data,
        1.25e5,
    ),
    def(
        "Gbit",
        &["Gb", "Gbit", "gigabit", "gigabits"],
        Dim::Data,
        1.25e8,
    ),
    def(
        "Tbit",
        &["Tb", "Tbit", "terabit", "terabits"],
        Dim::Data,
        1.25e11,
    ),
    // Speed (metre per second)
    def("m/s", &["m/s", "mps"], Dim::Speed, 1.0),
    def(
        "km/h",
        &["km/h", "kph", "kmh", "kmph"],
        Dim::Speed,
        1.0 / 3.6,
    ),
    def("mph", &["mph", "mi/h"], Dim::Speed, 0.447_04),
    def("ft/s", &["ft/s", "fps"], Dim::Speed, 0.3048),
    def(
        "kn",
        &["kn", "kt", "knot", "knots"],
        Dim::Speed,
        1852.0 / 3600.0,
    ),
    // Area (square metre)
    def("mm²", &["mm²", "mm2"], Dim::Area, 1e-6),
    def("cm²", &["cm²", "cm2"], Dim::Area, 1e-4),
    def("m²", &["m²", "m2", "sqm"], Dim::Area, 1.0),
    def("km²", &["km²", "km2"], Dim::Area, 1e6),
    def("ha", &["ha", "hectare", "hectares"], Dim::Area, 1e4),
    def("ac", &["ac", "acre", "acres"], Dim::Area, 4_046.856_422_4),
    def("in²", &["in²", "in2", "sqin"], Dim::Area, 0.000_645_16),
    def("ft²", &["ft²", "ft2", "sqft"], Dim::Area, 0.092_903_04),
    def("yd²", &["yd²", "yd2", "sqyd"], Dim::Area, 0.836_127_36),
    def("mi²", &["mi²", "mi2", "sqmi"], Dim::Area, 2_589_988.110_336),
    // Angle (radian)
    def(
        "rad",
        &["rad", "rads", "radian", "radians"],
        Dim::Angle,
        1.0,
    ),
    def(
        "°",
        &["°", "deg", "degree", "degrees"],
        Dim::Angle,
        std::f64::consts::PI / 180.0,
    ),
    def(
        "turn",
        &["turn", "turns"],
        Dim::Angle,
        std::f64::consts::TAU,
    ),
];

fn unit(def: &Def) -> Unit {
    Unit {
        symbol: def.symbol.to_owned(),
        dim: def.dim,
        factor: def.factor,
        offset: def.offset,
    }
}

/// The unit called `name`, if any (not currencies: see `currency`).
pub(super) fn lookup(name: &str) -> Option<Unit> {
    if let Some(def) = DEFS.iter().find(|d| d.names.contains(&name)) {
        return Some(unit(def));
    }
    DEFS.iter()
        .find(|d| d.names.iter().any(|n| n.eq_ignore_ascii_case(name)))
        .map(unit)
}

/// `sq ft`, `square metres`: the square of a length unit.
pub(super) fn square(length: &Unit) -> Option<Unit> {
    (length.dim == Dim::Length).then(|| Unit {
        symbol: format!("{}²", length.symbol),
        dim: Dim::Area,
        factor: length.factor * length.factor,
        offset: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(value: f64, from: &str, to: &str) -> f64 {
        let (from, to) = (lookup(from).unwrap(), lookup(to).unwrap());
        assert_eq!(from.dim, to.dim);
        to.value_from_base(from.to_base(value))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn converts_common_units() {
        assert!(close(convert(5.0, "km", "miles"), 3.106_855_961_186_156));
        assert!(close(convert(10.0, "kg", "lb"), 22.046_226_218_487_757));
        assert!(close(convert(1.0, "gal", "L"), 3.785_411_784));
        assert!(close(convert(1.0, "GB", "MB"), 1000.0));
        assert!(close(convert(1.0, "GiB", "MiB"), 1024.0));
        assert!(close(convert(100.0, "km/h", "mph"), 62.137_119_223_733_4));
    }

    #[test]
    fn temperatures_are_affine() {
        assert!(close(convert(100.0, "f", "c"), 37.777_777_777_777_78));
        assert!(close(convert(-40.0, "C", "F"), -40.0));
        assert!(close(convert(0.0, "C", "K"), 273.15));
    }

    #[test]
    fn case_matters_only_when_it_disambiguates() {
        assert_eq!(lookup("MB").unwrap().symbol, "MB");
        assert_eq!(lookup("Mb").unwrap().symbol, "Mbit");
        assert_eq!(lookup("mb").unwrap().symbol, "MB");
        assert_eq!(lookup("KM").unwrap().symbol, "km");
        assert!(lookup("password").is_none());
    }

    #[test]
    fn names_are_unique_per_case() {
        let mut seen = std::collections::HashSet::new();
        for def in DEFS {
            for name in def.names {
                assert!(seen.insert(*name), "{name} is defined twice");
            }
        }
    }

    #[test]
    fn squares_lengths() {
        let sq_ft = square(&lookup("ft").unwrap()).unwrap();
        assert_eq!(sq_ft.symbol, "ft²");
        assert!(close(sq_ft.factor, lookup("sqft").unwrap().factor));
        assert!(square(&lookup("kg").unwrap()).is_none());
    }
}
