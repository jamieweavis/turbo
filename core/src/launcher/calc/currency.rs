//! Currency names and exchange rates.
//!
//! Rates are the European Central Bank's daily reference rates, served without a key by
//! frankfurter.dev. They're cached in `<data_dir>/currency-rates.json` and refreshed in the
//! background every 12 hours; evaluation only ever reads the cache, so it never waits on the
//! network.
//!
//! Fetching uses Foundation (`NSData` from a URL) on the background thread rather than an HTTP
//! crate: it adds no TLS stack to the binary and honours the system's proxy and VPN settings.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock, Weak};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use objc2::rc::autoreleasepool;
use objc2_foundation::{NSData, NSDataReadingOptions, NSLocale, NSString, NSURL};
use serde::{Deserialize, Serialize};

use super::units::{Dim, Unit};

const RATES_URL: &str = "https://api.frankfurter.dev/v1/latest?base=EUR";
const REFRESH_AFTER: f64 = 12.0 * 3600.0;
/// After a failed fetch (offline, captive portal), try again sooner.
const RETRY_AFTER: f64 = 30.0 * 60.0;
/// The refresh thread wakes this often to notice the calculator has gone.
const POLL: Duration = Duration::from_secs(60);

/// The currencies the ECB publishes (plus EUR itself).
const CODES: &[&str] = &[
    "AUD", "BGN", "BRL", "CAD", "CHF", "CNY", "CZK", "DKK", "EUR", "GBP", "HKD", "HUF", "IDR",
    "ILS", "INR", "ISK", "JPY", "KRW", "MXN", "MYR", "NOK", "NZD", "PHP", "PLN", "RON", "SEK",
    "SGD", "THB", "TRY", "USD", "ZAR",
];

/// Prefix symbols, used both for input (`£20`) and output (`£16.75`). Ambiguous ones (`$`, `¥`)
/// mean the most common currency.
const SYMBOLS: &[(char, &str)] = &[
    ('$', "USD"),
    ('£', "GBP"),
    ('€', "EUR"),
    ('¥', "JPY"),
    ('₹', "INR"),
    ('₩', "KRW"),
];

/// Everyday names. "pound" is left to mass, where it's more often meant.
const NAMES: &[(&str, &str)] = &[
    ("dollar", "USD"),
    ("dollars", "USD"),
    ("buck", "USD"),
    ("bucks", "USD"),
    ("euro", "EUR"),
    ("euros", "EUR"),
    ("quid", "GBP"),
    ("sterling", "GBP"),
    ("yen", "JPY"),
    ("yuan", "CNY"),
    ("rmb", "CNY"),
    ("rupee", "INR"),
    ("rupees", "INR"),
    ("franc", "CHF"),
    ("francs", "CHF"),
    ("peso", "MXN"),
    ("pesos", "MXN"),
];

pub(super) fn is_symbol(c: char) -> bool {
    SYMBOLS.iter().any(|(s, _)| *s == c)
}

pub(super) fn code_for_symbol(c: char) -> Option<&'static str> {
    SYMBOLS.iter().find(|(s, _)| *s == c).map(|(_, code)| *code)
}

/// The ISO code for a word: a code in any case (`usd`, `GBP`) or a name (`euros`).
pub(super) fn code_for_word(word: &str) -> Option<&'static str> {
    let upper = word.to_ascii_uppercase();
    CODES
        .iter()
        .find(|code| **code == upper)
        .copied()
        .or_else(|| {
            let lower = word.to_ascii_lowercase();
            NAMES
                .iter()
                .find(|(n, _)| *n == lower)
                .map(|(_, code)| *code)
        })
}

pub(super) fn symbol_for_code(code: &str) -> Option<char> {
    SYMBOLS.iter().find(|(_, c)| *c == code).map(|(s, _)| *s)
}

/// A currency as a unit; its factor converts to euros. Without rates it's NaN, so any
/// calculation involving it comes out non-finite and isn't shown.
pub(super) fn unit(code: &str, rates: Option<&Rates>) -> Unit {
    let factor = rates
        .and_then(|r| r.per_eur.get(code))
        .map_or(f64::NAN, |rate| 1.0 / rate);
    Unit {
        symbol: code.to_owned(),
        dim: Dim::Currency,
        factor,
        offset: 0.0,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Rates {
    /// The ECB's publication date, e.g. `2026-10-23`.
    pub date: String,
    /// Units of each currency per euro (EUR included, at 1).
    pub per_eur: HashMap<String, f64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    /// Unix seconds of the fetch, for deciding when to refresh.
    fetched_at: f64,
    date: String,
    rates: HashMap<String, f64>,
}

#[derive(Deserialize)]
struct Response {
    date: String,
    rates: HashMap<String, f64>,
}

/// Parses frankfurter's `{"base":"EUR","date":…,"rates":{…}}`, rejecting anything unusable.
fn parse_response(bytes: &[u8]) -> Option<Rates> {
    let response: Response = serde_json::from_slice(bytes).ok()?;
    let mut per_eur: HashMap<String, f64> = response
        .rates
        .into_iter()
        .filter(|(_, rate)| rate.is_finite() && *rate > 0.0)
        .collect();
    if per_eur.is_empty() {
        return None;
    }
    per_eur.insert("EUR".into(), 1.0);
    Some(Rates {
        date: response.date,
        per_eur,
    })
}

/// The latest rates, kept fresh by a background thread.
pub(super) struct RateStore {
    rates: RwLock<Option<Arc<Rates>>>,
}

impl RateStore {
    /// Loads the cache and starts the refresh thread; returns immediately.
    pub fn start(data_dir: &Path) -> Arc<Self> {
        let store = Arc::new(Self {
            rates: RwLock::new(None),
        });
        let weak = Arc::downgrade(&store);
        let path = data_dir.join("currency-rates.json");
        let spawned = thread::Builder::new()
            .name("jaycast-currency-rates".into())
            .spawn(move || refresh_loop(&weak, &path));
        if let Err(error) = spawned {
            eprintln!("jaycast: couldn't start the currency rate thread: {error}");
        }
        store
    }

    pub fn current(&self) -> Option<Arc<Rates>> {
        self.rates.read().unwrap().clone()
    }

    fn set(&self, rates: Rates) {
        *self.rates.write().unwrap() = Some(Arc::new(rates));
    }
}

fn refresh_loop(store: &Weak<RateStore>, path: &Path) {
    let mut fetched_at = 0.0;
    if let Some(cache) = load(path) {
        fetched_at = cache.fetched_at;
        match store.upgrade() {
            Some(store) => store.set(Rates {
                date: cache.date,
                per_eur: cache.rates,
            }),
            None => return,
        }
    }

    let mut due = fetched_at + REFRESH_AFTER;
    loop {
        if now() >= due {
            match fetch() {
                Some(rates) => {
                    save(path, &rates);
                    match store.upgrade() {
                        Some(store) => store.set(rates),
                        None => return,
                    }
                    due = now() + REFRESH_AFTER;
                }
                None => due = now() + RETRY_AFTER,
            }
        }
        thread::sleep(POLL);
        if store.strong_count() == 0 {
            return;
        }
    }
}

fn load(path: &Path) -> Option<Cache> {
    let cache: Cache = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (!cache.rates.is_empty()).then_some(cache)
}

/// Atomic like the config: a crash mid-write never leaves a broken cache.
fn save(path: &Path, rates: &Rates) {
    let cache = Cache {
        fetched_at: now(),
        date: rates.date.clone(),
        rates: rates.per_eur.clone(),
    };
    let Ok(json) = serde_json::to_vec_pretty(&cache) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, json).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Blocks for the request; only ever called on the refresh thread.
fn fetch() -> Option<Rates> {
    let bytes = autoreleasepool(|_| {
        let url = NSURL::URLWithString(&NSString::from_str(RATES_URL))?;
        let data = NSData::dataWithContentsOfURL_options_error(&url, NSDataReadingOptions::empty())
            .ok()?;
        Some(data.to_vec())
    })?;
    parse_response(&bytes)
}

/// The user's local currency (from their region settings), if it's one we have rates for.
pub(super) fn local_currency() -> Option<String> {
    autoreleasepool(|_| {
        let code = NSLocale::currentLocale().currencyCode()?.to_string();
        CODES.contains(&code.as_str()).then_some(code)
    })
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
pub(super) fn test_rates() -> Rates {
    let per_eur = [
        ("EUR", 1.0),
        ("USD", 1.25),
        ("GBP", 0.8),
        ("JPY", 160.0),
        ("CHF", 0.95),
    ]
    .into_iter()
    .map(|(code, rate)| (code.to_owned(), rate))
    .collect();
    Rates {
        date: "2026-10-23".into(),
        per_eur,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_api_response() {
        let json = br#"{"amount":1.0,"base":"EUR","date":"2026-10-23","rates":{"GBP":0.83728,"USD":1.16,"BAD":0}}"#;
        let rates = parse_response(json).unwrap();
        assert_eq!(rates.date, "2026-10-23");
        assert_eq!(rates.per_eur["EUR"], 1.0);
        assert_eq!(rates.per_eur["GBP"], 0.83728);
        assert!(!rates.per_eur.contains_key("BAD"));
        assert!(parse_response(b"<html>").is_none());
        assert!(parse_response(br#"{"date":"x","rates":{}}"#).is_none());
    }

    #[test]
    fn recognises_codes_names_and_symbols() {
        assert_eq!(code_for_word("usd"), Some("USD"));
        assert_eq!(code_for_word("Euros"), Some("EUR"));
        assert_eq!(code_for_word("yen"), Some("JPY"));
        assert_eq!(code_for_word("pounds"), None);
        assert_eq!(code_for_word("xyz"), None);
        assert_eq!(code_for_symbol('£'), Some("GBP"));
        assert_eq!(symbol_for_code("EUR"), Some('€'));
        assert_eq!(symbol_for_code("CHF"), None);
    }

    #[test]
    fn units_convert_through_euros() {
        let rates = test_rates();
        let (usd, gbp) = (unit("USD", Some(&rates)), unit("GBP", Some(&rates)));
        assert!((gbp.value_from_base(usd.to_base(100.0)) - 64.0).abs() < 1e-9);
        assert!(unit("USD", None).factor.is_nan());
    }

    #[test]
    fn cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("jaycast-rates-{}", std::process::id()));
        let path = dir.join("currency-rates.json");
        save(&path, &test_rates());
        let cache = load(&path).unwrap();
        assert_eq!(cache.date, "2026-10-23");
        assert_eq!(cache.rates["GBP"], 0.8);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Hits the network: `cargo test -- --ignored live_rates`.
    #[test]
    #[ignore]
    fn live_rates() {
        let rates = fetch().expect("fetch failed");
        println!("{} rates from {}", rates.per_eur.len(), rates.date);
        assert!(rates.per_eur.contains_key("USD"));
    }
}
