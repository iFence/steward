//! Search, indexing, fuzzy matching and ranking.
//!
//! No UI dependencies: everything here is unit-testable in isolation.
//!
//! M1 adds application scanning (Windows-first) and `nucleo`-based fuzzy
//! matching with usage-frequency weighting.

use std::cell::RefCell;
use std::path::PathBuf;

use nucleo::{Config, Matcher, Utf32Str, Utf32String};
use serde::{Deserialize, Serialize};

pub use nucleo;

mod scanner;

pub use scanner::{platform_scanner, AppScanner};

/// A single installed application discoverable via the launcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppEntry {
    pub name: String,
    pub path: PathBuf,
}

/// Weight applied to the natural log of (1 + usage count) when ranking.
/// Keeps a single boost reasonable relative to nucleo's fuzzy scores (which
/// are typically in the low hundreds to thousands).
const FREQ_WEIGHT: f64 = 20.0;

/// A candidate produced by `Engine::query`, carrying the fuzzy score and the
/// app it corresponds to.
#[derive(Debug)]
struct ScoredApp {
    app: AppEntry,
    score: u16,
}

/// The search engine: an immutable index of applications plus a reusable
/// `nucleo` matcher. Querying borrows `&self`; haystack buffers are pre-built
/// once per entry and re-read on each query.
pub struct Engine {
    entries: Vec<AppEntry>,
    haystacks: RefCell<Vec<Utf32String>>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            haystacks: RefCell::new(Vec::new()),
        }
    }

    /// Rebuild the index from a fresh scan result.
    pub fn set_entries(&mut self, entries: Vec<AppEntry>) {
        self.haystacks = RefCell::new(
            entries
                .iter()
                .map(|e| Utf32String::from(e.name.as_str()))
                .collect(),
        );
        self.entries = entries;
    }

    pub fn entries(&self) -> &[AppEntry] {
        &self.entries
    }

    /// Fuzzy-match `query` against the index. An empty/blank query returns all
    /// entries sorted only by usage frequency (most-used first).
    ///
    /// `freq` resolves the usage count for an app path; pass a no-op closure
    /// when usage is unknown.
    pub fn query(&self, query: &str, freq: &dyn Fn(&str) -> u32) -> Vec<AppEntry> {
        // nucleo's default config is case-insensitive with latin normalization,
        // which is well suited to launcher search.
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut needle_buf = Vec::new();
        let needle = Utf32Str::new(query, &mut needle_buf);
        let haystacks = self.haystacks.borrow();

        let mut scored: Vec<ScoredApp> = if query.trim().is_empty() {
            self.entries
                .iter()
                .zip(haystacks.iter())
                .map(|(app, _)| ScoredApp {
                    app: app.clone(),
                    score: 0,
                })
                .collect()
        } else {
            let mut matches = Vec::new();
            for (app, haystack) in self.entries.iter().zip(haystacks.iter()) {
                let hay: Utf32Str<'_> = haystack.slice(..);
                if let Some(score) = matcher.fuzzy_match(hay, needle) {
                    matches.push(ScoredApp {
                        app: app.clone(),
                        score,
                    });
                }
            }
            matches
        };

        let freq = |app: &AppEntry| freq(&self.path_key(app));
        let blank = query.trim().is_empty();
        scored.sort_by(|a, b| {
            // Most-used first for empty queries, else rank by weighted score.
            let (sa, sb) = if blank {
                (a.frequency(&freq), b.frequency(&freq))
            } else {
                (a.rank(&freq), b.rank(&freq))
            };
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });

        scored.into_iter().map(|s| s.app).collect()
    }

    fn path_key(&self, app: &AppEntry) -> String {
        app.path.to_string_lossy().into_owned()
    }
}

impl ScoredApp {
    fn frequency(&self, freq: &dyn Fn(&AppEntry) -> u32) -> f64 {
        freq(&self.app) as f64
    }

    fn rank(&self, freq: &dyn Fn(&AppEntry) -> u32) -> f64 {
        let count = freq(&self.app) as f64;
        (self.score as f64) + FREQ_WEIGHT * (1.0 + count).ln()
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entries() -> Vec<AppEntry> {
        vec![
            AppEntry {
                name: "Calculator".into(),
                path: PathBuf::from("C:/calc.exe"),
            },
            AppEntry {
                name: "Firefox".into(),
                path: PathBuf::from("C:/firefox.exe"),
            },
            AppEntry {
                name: "Calculator Pro".into(),
                path: PathBuf::from("D:/calcpro.exe"),
            },
            AppEntry {
                name: "Terminal".into(),
                path: PathBuf::from("C:/terminal.exe"),
            },
        ]
    }

    const NO_FREQ: fn(&str) -> u32 = |_| 0;

    #[test]
    fn empty_query_returns_all_entries() {
        let mut engine = Engine::new();
        engine.set_entries(entries());
        assert_eq!(engine.query("", &NO_FREQ).len(), 4);
        assert_eq!(engine.query("   ", &NO_FREQ).len(), 4);
    }

    #[test]
    fn fuzzy_matches_subsequence_ignoring_case() {
        let mut engine = Engine::new();
        engine.set_entries(entries());
        let result = engine.query("calc", &NO_FREQ);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|a| a.name.contains("Calculator")));
    }

    #[test]
    fn lowercase_query_matches_ignoring_case() {
        let mut engine = Engine::new();
        engine.set_entries(entries());
        // nucleo default config is case-insensitive.
        assert_eq!(engine.query("firefox", &NO_FREQ).len(), 1);
    }

    #[test]
    fn no_match_on_empty_index() {
        let engine = Engine::new();
        assert!(engine.query("calc", &NO_FREQ).is_empty());
    }

    #[test]
    fn frequent_app_ranks_higher() {
        let mut engine = Engine::new();
        engine.set_entries(entries());
        let freq = |path: &str| if path == "D:/calcpro.exe" { 50 } else { 0 };
        let result = engine.query("calc", &freq);
        assert_eq!(result[0].path.to_string_lossy(), "D:/calcpro.exe");
    }

    #[test]
    fn empty_query_sorts_most_used_first() {
        let mut engine = Engine::new();
        engine.set_entries(entries());
        let freq = |path: &str| match path {
            "C:/terminal.exe" => 30,
            "C:/firefox.exe" => 10,
            _ => 0,
        };
        let result = engine.query("", &freq);
        assert_eq!(result[0].name, "Terminal");
        assert_eq!(result[1].name, "Firefox");
    }
}
