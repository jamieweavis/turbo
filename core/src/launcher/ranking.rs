//! Usage-based ranking: results you open often, recently, or for the same query, rise.
//!
//! Two decaying counters, both halving every [`HALF_LIFE`]:
//! - **frecency** per result: every open adds 1, so frequent *and* recent wins;
//! - **query affinity** per (typed prefix, result): opening Safari after typing "saf" also
//!   credits "s" and "sa", so next time "s" alone puts Safari first.
//!
//! Boosts are capped so a strong fuzzy match still beats a weak match you merely use a lot,
//! while a result you've explicitly chosen for this query can overtake a slightly better
//! text match. Time is always a parameter (seconds since the Unix epoch), so this is pure.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// A week, in seconds.
const HALF_LIFE: f64 = 7.0 * 24.0 * 60.0 * 60.0;
/// Frecency boost: `FRECENCY_WEIGHT × ln(1 + frecency)`, capped. One recent open ≈ 6,
/// five ≈ 14, a dozen or more = the cap: less than the gap between a word-start match and
/// a mid-word one, so match quality wins over habit.
const FRECENCY_WEIGHT: f64 = 8.0;
const FRECENCY_CAP: f64 = 20.0;
/// Query boost: `QUERY_WEIGHT × affinity`, capped. One choice for this prefix ≈ 40.
const QUERY_WEIGHT: f64 = 40.0;
const QUERY_CAP: f64 = 60.0;
/// Prefixes longer than this aren't remembered; they're specific enough already.
const MAX_QUERY_KEY: usize = 24;
/// Counters that have decayed below this are dropped when saving.
const PRUNE_BELOW: f64 = 0.02;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
struct Decayed {
    value: f64,
    /// When `value` was last brought up to date.
    at: f64,
}

impl Decayed {
    fn value_at(&self, now: f64) -> f64 {
        let elapsed = (now - self.at).max(0.0);
        self.value * 0.5_f64.powf(elapsed / HALF_LIFE)
    }

    fn bump(&mut self, now: f64) {
        self.value = self.value_at(now) + 1.0;
        self.at = now;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Usage {
    items: HashMap<String, Decayed>,
    /// Folded query prefix → result id → affinity.
    queries: HashMap<String, HashMap<String, Decayed>>,
}

impl Usage {
    /// Records that `id` was opened after typing `query` (already folded).
    pub fn record(&mut self, id: &str, query: &[char], now: f64) {
        self.items.entry(id.to_owned()).or_default().bump(now);
        let query: Vec<char> = query
            .iter()
            .copied()
            .filter(|c| !c.is_whitespace())
            .collect();
        for len in 1..=query.len().min(MAX_QUERY_KEY) {
            let key: String = query[..len].iter().collect();
            self.queries
                .entry(key)
                .or_default()
                .entry(id.to_owned())
                .or_default()
                .bump(now);
        }
    }

    pub fn frecency(&self, id: &str, now: f64) -> f64 {
        self.items.get(id).map_or(0.0, |d| d.value_at(now))
    }

    /// The ranking boost for `id` when `query` (folded) is typed.
    pub fn boost(&self, id: &str, query: &[char], now: f64) -> i32 {
        let frecency = (FRECENCY_WEIGHT * self.frecency(id, now).ln_1p()).min(FRECENCY_CAP);
        let key: String = query
            .iter()
            .copied()
            .filter(|c| !c.is_whitespace())
            .take(MAX_QUERY_KEY)
            .collect();
        let affinity = self
            .queries
            .get(&key)
            .and_then(|ids| ids.get(id))
            .map_or(0.0, |d| d.value_at(now));
        let query_boost = (QUERY_WEIGHT * affinity).min(QUERY_CAP);
        (frecency + query_boost).round() as i32
    }

    /// Ids with any history, most frecent first.
    pub fn most_frecent(&self, now: f64) -> Vec<(String, f64)> {
        let mut ids: Vec<(String, f64)> = self
            .items
            .iter()
            .map(|(id, d)| (id.clone(), d.value_at(now)))
            .filter(|(_, f)| *f >= PRUNE_BELOW)
            .collect();
        ids.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ids
    }

    /// Drops counters that have decayed to nothing, so the file doesn't grow forever.
    fn prune(&mut self, now: f64) {
        self.items.retain(|_, d| d.value_at(now) >= PRUNE_BELOW);
        for ids in self.queries.values_mut() {
            ids.retain(|_, d| d.value_at(now) >= PRUNE_BELOW);
        }
        self.queries.retain(|_, ids| !ids.is_empty());
    }
}

/// Loads and saves [`Usage`] at `<data_dir>/launcher-usage.json`.
pub(crate) struct UsageStore {
    path: PathBuf,
}

impl UsageStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("launcher-usage.json"),
        }
    }

    /// Missing or unreadable history just means starting fresh: it's only a ranking hint,
    /// unlike settings, which are never silently discarded.
    pub fn load(&self) -> Usage {
        fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Prunes, then writes atomically (temp file + rename).
    pub fn save(&self, usage: &Usage, now: f64) -> Result<(), CoreError> {
        let mut usage = usage.clone();
        usage.prune(now);
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(&usage)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f64 = 24.0 * 60.0 * 60.0;

    fn q(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn no_history_means_no_boost() {
        let usage = Usage::default();
        assert_eq!(usage.boost("app:a", &q("a"), 0.0), 0);
        assert!(usage.most_frecent(0.0).is_empty());
    }

    #[test]
    fn frecency_halves_every_week() {
        let mut usage = Usage::default();
        usage.record("app:a", &q(""), 0.0);
        assert!((usage.frecency("app:a", 0.0) - 1.0).abs() < 1e-9);
        assert!((usage.frecency("app:a", 7.0 * DAY) - 0.5).abs() < 1e-9);
        assert!((usage.frecency("app:a", 14.0 * DAY) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn frequent_and_recent_both_count() {
        let mut usage = Usage::default();
        for day in 0..5 {
            usage.record("app:often", &q(""), f64::from(day) * DAY);
        }
        usage.record("app:once", &q(""), 4.0 * DAY);
        let now = 5.0 * DAY;
        assert!(usage.frecency("app:often", now) > usage.frecency("app:once", now));
        let ranked: Vec<_> = usage
            .most_frecent(now)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ranked, vec!["app:often", "app:once"]);
    }

    #[test]
    fn choosing_after_a_query_credits_its_prefixes() {
        let mut usage = Usage::default();
        usage.record("app:safari", &q("saf"), 0.0);
        let s = usage.boost("app:safari", &q("s"), 0.0);
        let other = usage.boost("app:settings", &q("s"), 0.0);
        assert!(s > other);
        assert!(
            s >= 40,
            "one choice gives the query boost on top of frecency: {s}"
        );
        // A longer query than was typed has no affinity, only frecency.
        assert!(usage.boost("app:safari", &q("safari"), 0.0) < s);
    }

    #[test]
    fn boosts_are_capped() {
        let mut usage = Usage::default();
        for _ in 0..1000 {
            usage.record("app:a", &q("a"), 0.0);
        }
        assert_eq!(
            usage.boost("app:a", &q("a"), 0.0),
            (FRECENCY_CAP + QUERY_CAP) as i32
        );
    }

    #[test]
    fn whitespace_is_ignored_in_query_keys() {
        let mut usage = Usage::default();
        usage.record("app:ss", &q("sys set"), 0.0);
        assert!(usage.boost("app:ss", &q("sysset"), 0.0) > usage.boost("app:ss", &q("x"), 0.0));
    }

    #[test]
    fn store_round_trips_and_prunes() {
        let dir = std::env::temp_dir().join(format!("jaycast-usage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = UsageStore::new(&dir);
        assert_eq!(store.load(), Usage::default());

        let mut usage = Usage::default();
        usage.record("app:old", &q("o"), 0.0);
        usage.record("app:new", &q("n"), 100.0 * DAY);
        store.save(&usage, 100.0 * DAY).unwrap();
        let loaded = store.load();
        assert!(loaded.frecency("app:new", 100.0 * DAY) > 0.9);
        assert_eq!(loaded.frecency("app:old", 100.0 * DAY), 0.0, "pruned");
        assert!(!loaded.queries.contains_key("o"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_file_starts_fresh() {
        let dir = std::env::temp_dir().join(format!("jaycast-usage-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("launcher-usage.json"), b"{ not json").unwrap();
        assert_eq!(UsageStore::new(&dir).load(), Usage::default());
        fs::remove_dir_all(dir).unwrap();
    }
}
