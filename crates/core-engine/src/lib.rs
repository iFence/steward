//! Search, indexing, fuzzy matching and ranking.
//!
//! No UI dependencies: everything here is unit-testable in isolation.
//!
//! M1 adds application scanning (Windows-first) and `nucleo`-based fuzzy
//! matching with usage-frequency weighting.

use std::cell::RefCell;
use std::path::PathBuf;

use nucleo::{Config, Matcher, Utf32Str, Utf32String};
use pinyin::ToPinyinMulti;
use serde::{Deserialize, Serialize};

pub use nucleo;

mod scanner;

pub mod calc;
pub mod link;

pub use calc::{format_value, try_evaluate};
pub use link::try_openable;
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
    /// Per-entry haystack variants to fuzzy-match against: the display name
    /// plus, for Chinese names, pinyin spellings (see [`pinyin_variants`]).
    haystacks: RefCell<Vec<Vec<Utf32String>>>,
    /// Reusable fuzzy matcher kept across queries so per-keystroke allocation
    /// stays flat instead of building a fresh matcher every time.
    matcher: RefCell<Matcher>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            haystacks: RefCell::new(Vec::new()),
            matcher: RefCell::new(Matcher::new(Config::DEFAULT)),
        }
    }

    /// Rebuild the index from a fresh scan result.
    pub fn set_entries(&mut self, entries: Vec<AppEntry>) {
        self.haystacks = RefCell::new(entries.iter().map(|e| search_haystacks(&e.name)).collect());
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
        let mut matcher = self.matcher.borrow_mut();
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
            for (app, variants) in self.entries.iter().zip(haystacks.iter()) {
                // Match against every search form (name, full pinyin, spaced
                // pinyin, initials) and keep the best score for the entry.
                let mut best: Option<u16> = None;
                for haystack in variants {
                    let hay: Utf32Str<'_> = haystack.slice(..);
                    if let Some(score) = matcher.fuzzy_match(hay, needle) {
                        best = Some(best.map_or(score, |current| current.max(score)));
                    }
                }
                if let Some(score) = best {
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

/// Build the search haystack variants for a label or keyword: the text itself
/// plus, for Chinese text, compact full pinyin (`zidongbofang`), full pinyin
/// with spaces (`zi dong bo fang`) and first-letter initials (`zdbf`), so
/// queries like `zd` can find 终端 / 自动播放 / 设置向导 the way other launchers
/// do. Characters without a pinyin reading (Latin, digits, punctuation) are
/// copied through, keeping mixed names such as "QQ音乐" searchable by either
/// part. App names and plugin keywords share this matcher vocabulary.
pub fn search_haystacks(name: &str) -> Vec<Utf32String> {
    let mut seen = std::collections::HashSet::new();
    let mut variants = Vec::new();
    for text in std::iter::once(name.to_owned()).chain(pinyin_variants(name)) {
        if seen.insert(text.clone()) {
            variants.push(Utf32String::from(text));
        }
    }
    variants
}

/// Pinyin search forms for `name`. Pure-Latin names yield no extra variants
/// (the caller keeps the original name, and nucleo is case-insensitive).
fn pinyin_variants(name: &str) -> Vec<String> {
    let mut full_variants = vec![String::new()];
    let mut spaced_variants = vec![String::new()];
    let mut initials_variants = vec![String::new()];

    for ch in name.chars() {
        if let Some(multi) = ch.to_pinyin_multi() {
            let mut readings = Vec::new();
            let mut letters = Vec::new();
            for syllable in multi {
                readings.push(syllable.plain());
                letters.push(syllable.first_letter());
            }
            // Cartesian product over all readings (polyphones like 乐 have
            // yue/le, 行 has xing/hang), capped so names with several
            // polyphones cannot blow up the index.
            full_variants =
                expand_pinyin(full_variants, &readings, |acc, part| format!("{acc}{part}"));
            spaced_variants = expand_pinyin(spaced_variants, &readings, |acc, part| {
                if acc.is_empty() {
                    part.to_string()
                } else {
                    format!("{acc} {part}")
                }
            });
            initials_variants = expand_pinyin(initials_variants, &letters, |acc, part| {
                format!("{acc}{part}")
            });
        } else if !ch.is_whitespace() {
            let c = ch.to_ascii_lowercase();
            for variant in &mut full_variants {
                variant.push(c);
            }
            for variant in &mut spaced_variants {
                if !variant.is_empty() {
                    variant.push(' ');
                }
                variant.push(c);
            }
            for variant in &mut initials_variants {
                variant.push(c);
            }
        }
    }

    let lowercase = name.to_ascii_lowercase();
    let mut variants = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for text in full_variants
        .into_iter()
        .chain(spaced_variants)
        .chain(initials_variants)
    {
        if text.is_empty() || text == lowercase {
            continue;
        }
        if seen.insert(text.clone()) {
            variants.push(text);
        }
        if variants.len() >= MAX_PINYIN_HANSTACKS {
            break;
        }
    }
    variants
}

/// Upper bound on pinyin search forms generated per app name.
const MAX_PINYIN_HANSTACKS: usize = 12;

/// Combine every existing variant with every reading, stopping once
/// [`MAX_PINYIN_HANSTACKS`] is reached.
fn expand_pinyin(
    variants: Vec<String>,
    parts: &[&str],
    combine: impl Fn(String, &str) -> String,
) -> Vec<String> {
    let mut out = Vec::with_capacity(
        variants
            .len()
            .saturating_mul(parts.len())
            .min(MAX_PINYIN_HANSTACKS),
    );
    'outer: for variant in variants {
        for part in parts {
            if out.len() >= MAX_PINYIN_HANSTACKS {
                break 'outer;
            }
            out.push(combine(variant.clone(), part));
        }
    }
    out
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

    fn chinese_entries() -> Vec<AppEntry> {
        vec![
            AppEntry {
                name: "终端".into(),
                path: PathBuf::from("C:/terminal-cn.lnk"),
            },
            AppEntry {
                name: "自动播放".into(),
                path: PathBuf::from("C:/autoplay-cn.lnk"),
            },
            AppEntry {
                name: "设置向导".into(),
                path: PathBuf::from("C:/wizard-cn.lnk"),
            },
            AppEntry {
                name: "QQ音乐".into(),
                path: PathBuf::from("C:/qqmusic-cn.lnk"),
            },
            AppEntry {
                name: "Steward".into(),
                path: PathBuf::from("C:/steward.exe"),
            },
        ]
    }

    #[test]
    fn pinyin_initials_match_chinese_names() {
        let mut engine = Engine::new();
        engine.set_entries(chinese_entries());
        let result = engine.query("zd", &NO_FREQ);
        let names: Vec<_> = result.iter().map(|a| a.name.as_str()).collect();
        assert!(
            names.contains(&"终端"),
            "zd should find 终端, got {names:?}"
        );
        assert!(
            names.contains(&"自动播放"),
            "zd should find 自动播放, got {names:?}"
        );
        assert!(
            names.contains(&"设置向导"),
            "zd should find 设置向导, got {names:?}"
        );
    }

    #[test]
    fn pinyin_full_spelling_matches() {
        let mut engine = Engine::new();
        engine.set_entries(chinese_entries());
        assert!(engine
            .query("zidong", &NO_FREQ)
            .iter()
            .any(|a| a.name == "自动播放"));
        assert!(engine
            .query("shezhi", &NO_FREQ)
            .iter()
            .any(|a| a.name == "设置向导"));
        assert!(engine
            .query("zhongduan", &NO_FREQ)
            .iter()
            .any(|a| a.name == "终端"));
    }

    #[test]
    fn pinyin_with_spaces_matches() {
        let mut engine = Engine::new();
        engine.set_entries(chinese_entries());
        assert!(engine
            .query("zi dong", &NO_FREQ)
            .iter()
            .any(|a| a.name == "自动播放"));
    }

    #[test]
    fn mixed_latin_chinese_matches_by_both_parts() {
        let mut engine = Engine::new();
        engine.set_entries(chinese_entries());
        assert!(engine
            .query("qq", &NO_FREQ)
            .iter()
            .any(|a| a.name == "QQ音乐"));
        assert!(engine
            .query("yy", &NO_FREQ)
            .iter()
            .any(|a| a.name == "QQ音乐"));
    }

    #[test]
    fn polyphone_names_match_all_readings() {
        let mut engine = Engine::new();
        engine.set_entries(chinese_entries());
        // 乐 is polyphonic: yue (music) and le (happy). Both readings must be
        // searchable so "QQ音乐" is found by its actual pronunciation.
        assert!(engine
            .query("yinyue", &NO_FREQ)
            .iter()
            .any(|a| a.name == "QQ音乐"));
        assert!(engine
            .query("yinle", &NO_FREQ)
            .iter()
            .any(|a| a.name == "QQ音乐"));
    }

    #[test]
    fn pinyin_query_does_not_match_unrelated_latin_names() {
        let mut engine = Engine::new();
        engine.set_entries(chinese_entries());
        // "zd" has no z/d in "Steward" (or "Firefox"), so pure-Latin entries
        // must not leak into pinyin results.
        assert!(!engine
            .query("zd", &NO_FREQ)
            .iter()
            .any(|a| a.name == "Steward"));
    }
}
