//! Shared search vocabulary for applications and plugin routes.

use std::collections::HashSet;

use nucleo::Utf32String;
use pinyin::ToPinyinMulti;

/// Upper bound on pinyin search forms generated for one label.
const MAX_PINYIN_HAYSTACKS: usize = 12;

/// Build the search haystack variants for a label or keyword.
///
/// The original text is followed by compact full pinyin, spaced pinyin, and
/// first-letter initials for Chinese text. Characters without a pinyin
/// reading are copied through so mixed names remain searchable by either
/// component.
pub fn search_haystacks(name: &str) -> Vec<Utf32String> {
    let mut seen = HashSet::new();
    let mut variants = Vec::new();
    for text in std::iter::once(name.to_owned()).chain(pinyin_variants(name)) {
        if seen.insert(text.clone()) {
            variants.push(Utf32String::from(text));
        }
    }
    variants
}

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
            let ch = ch.to_ascii_lowercase();
            for variant in &mut full_variants {
                variant.push(ch);
            }
            for variant in &mut spaced_variants {
                if !variant.is_empty() {
                    variant.push(' ');
                }
                variant.push(ch);
            }
            for variant in &mut initials_variants {
                variant.push(ch);
            }
        }
    }

    let lowercase = name.to_ascii_lowercase();
    let mut variants = Vec::new();
    let mut seen = HashSet::new();
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
        if variants.len() >= MAX_PINYIN_HAYSTACKS {
            break;
        }
    }
    variants
}

fn expand_pinyin(
    variants: Vec<String>,
    parts: &[&str],
    combine: impl Fn(String, &str) -> String,
) -> Vec<String> {
    let mut out = Vec::with_capacity(
        variants
            .len()
            .saturating_mul(parts.len())
            .min(MAX_PINYIN_HAYSTACKS),
    );
    'outer: for variant in variants {
        for part in parts {
            if out.len() >= MAX_PINYIN_HAYSTACKS {
                break 'outer;
            }
            out.push(combine(variant.clone(), part));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(name: &str) -> Vec<String> {
        search_haystacks(name)
            .into_iter()
            .map(|text| text.to_string())
            .collect()
    }

    #[test]
    fn chinese_names_include_full_spaced_and_initial_pinyin() {
        let variants = texts("自动播放");
        assert!(variants.iter().any(|variant| variant == "zidongbofang"));
        assert!(variants.iter().any(|variant| variant == "zi dong bo fang"));
        assert!(variants.iter().any(|variant| variant == "zdbf"));
    }

    #[test]
    fn latin_names_keep_the_original_haystack() {
        let variants = texts("Calculator");
        assert_eq!(variants.first().map(String::as_str), Some("Calculator"));
        assert!(variants.len() <= MAX_PINYIN_HAYSTACKS + 1);
    }
}
