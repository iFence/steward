//! Query compilation: the "parse terms and modifiers, then match in memory"
//! half of the recovered design (report §6.1).
//!
//! The original parser understands `case:`, `nocase:`, `path:`, `regex:`,
//! `wildcards:`, `wholeword:`, `diacritics:` modifiers, `< >` groups, negation
//! and branch connections. This module implements that surface minus
//! `diacritics:` (an ASCII-folded haystack already covers the common case, and
//! a real implementation needs Unicode decomposition), and adds the metadata
//! filters a file launcher wants (`ext:`, `size:`, `dm:`, `type:`,
//! `folder:`).
//!
//! Grammar, in one line: whitespace-separated terms are **and**-ed, a `< ... >`
//! group is **or**-ed, a leading `!` or `-` negates a term, and `modifier:value`
//! scopes a single term while a bare `modifier:` scopes everything after it.
//! `"quoted text"` keeps its spaces.

use regex::Regex;

/// How a text term is compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatchMode {
    /// Plain substring (`report` finds `annual-report.pdf`).
    #[default]
    Substring,
    /// Substring bounded by non-word characters (`report` does not match
    /// `reporting`).
    WholeWord,
    /// `*` and `?` globbing, where `*` also crosses separators.
    Wildcards,
    /// Regular expression (`regex` crate; the original links PCRE).
    Regex,
}

/// Which text a term is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// The file name only (the default, and the cheap path).
    #[default]
    Name,
    /// The full path (needs the parent chain, so it costs more per record).
    Path,
}

/// Case sensitivity of a term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseMode {
    /// Compare with ASCII folding (the default).
    #[default]
    Folded,
    /// Compare exactly.
    Sensitive,
}

/// A single text term.
#[derive(Debug, Clone)]
pub struct TextPredicate {
    /// The literal text (for regex, the pattern source).
    pub text: String,
    pub mode: MatchMode,
    pub scope: Scope,
    pub case: CaseMode,
    /// Compiled regex for [`MatchMode::Regex`], built once per query.
    regex: Option<Regex>,
}

/// Two text predicates are equal when they would match the same inputs.
impl PartialEq for TextPredicate {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.mode == other.mode
            && self.scope == other.scope
            && self.case == other.case
    }
}

impl Eq for TextPredicate {}

impl TextPredicate {
    /// Whether this predicate can only be answered with the full path.
    pub fn needs_path(&self) -> bool {
        self.scope == Scope::Path
    }

    /// Whether the (folded) needle occurs in `haystack`.
    fn matches(&self, haystack: &[u8]) -> bool {
        match self.mode {
            MatchMode::Regex => {
                let Some(regex) = self.regex.as_ref() else {
                    return false;
                };
                match std::str::from_utf8(haystack) {
                    Ok(text) => regex.is_match(text),
                    // A lossy copy keeps matching possible for odd names.
                    Err(_) => regex.is_match(&String::from_utf8_lossy(haystack)),
                }
            }
            MatchMode::Wildcards => wildcard_match(haystack, self.text.as_bytes(), self.case),
            MatchMode::WholeWord => whole_word_match(haystack, self.text.as_bytes(), self.case),
            MatchMode::Substring => substring_match(haystack, self.text.as_bytes(), self.case),
        }
    }
}

/// Size filter (`size:>10M`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeFilter {
    pub op: Compare,
    pub bytes: u64,
}

/// A modification-time filter (`dm:today`, `dm:>2024-01-01`), stored as a
/// threshold in the index's own mtime representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateFilter {
    pub op: Compare,
    pub mtime: u64,
}

/// Comparison operator shared by the metadata filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl Compare {
    fn apply(self, value: u64, threshold: u64) -> bool {
        match self {
            Self::Less => value < threshold,
            Self::LessOrEqual => value <= threshold,
            Self::Greater => value > threshold,
            Self::GreaterOrEqual => value >= threshold,
        }
    }
}

/// Which record kinds a query accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindFilter {
    Any,
    FilesOnly,
    FoldersOnly,
}

/// One matching condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    Text(TextPredicate),
    /// Case-insensitive extension match without the dot.
    Extension(String),
    Size(SizeFilter),
    Modified(DateFilter),
    Kind(KindFilter),
}

impl Predicate {
    /// Whether evaluating this predicate requires the full path.
    pub fn needs_path(&self) -> bool {
        matches!(self, Self::Text(text) if text.needs_path())
    }
}

/// The compiled query: a small expression tree over [`Predicate`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    /// No positive term: everything matches (subject to the kind filter).
    Empty,
    Pred(Predicate),
    Not(Box<Filter>),
    All(Vec<Filter>),
    Any(Vec<Filter>),
}

/// Flatten a conjunction into its members, descending through nested `All`
/// nodes but not through `Any` groups (an alternative is not a conjunct).
fn collect_conjuncts<'a>(filter: &'a Filter, out: &mut Vec<&'a Filter>) {
    match filter {
        Filter::All(items) => {
            for item in items {
                collect_conjuncts(item, out);
            }
        }
        other => out.push(other),
    }
}

impl Filter {
    /// Whether the whole filter is satisfied by nothing, so no record can ever
    /// match and the scan can be skipped. Detects both halves of a
    /// contradiction added separately (`a !a`) and one nested in a group.
    pub fn is_never(&self) -> bool {
        match self {
            Self::Pred(..) | Self::Empty => false,
            Self::Not(inner) => inner.is_always(),
            Self::All(items) => {
                let mut seen: Vec<&Filter> = Vec::new();
                collect_conjuncts(self, &mut seen);
                seen.iter().any(|item| {
                    matches!(item, Filter::Pred(..))
                        && seen
                            .iter()
                            .any(|other| matches!(other, Filter::Not(inner) if **inner == **item))
                }) || items.iter().any(Filter::is_never)
            }
            Self::Any(items) => !items.is_empty() && items.iter().all(Filter::is_never),
        }
    }

    /// Whether the filter is satisfied by everything.
    pub fn is_always(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Pred(..) => false,
            Self::Not(inner) => inner.is_never(),
            Self::All(items) => items.iter().all(Filter::is_always),
            Self::Any(items) => items.is_empty() || items.iter().any(Filter::is_always),
        }
    }

    /// Whether any predicate in the tree needs the full path, which is how the
    /// searcher decides whether to materialise paths per record.
    pub fn needs_path(&self) -> bool {
        match self {
            Self::Empty => false,
            Self::Pred(predicate) => predicate.needs_path(),
            Self::Not(inner) => inner.needs_path(),
            Self::All(items) | Self::Any(items) => items.iter().any(Filter::needs_path),
        }
    }

    /// Kind filter requested by the query, if exactly one applies.
    pub fn kind(&self) -> KindFilter {
        fn find(filter: &Filter) -> Option<KindFilter> {
            match filter {
                Filter::Pred(Predicate::Kind(kind)) => Some(*kind),
                Filter::All(items) | Filter::Any(items) => items.iter().find_map(find),
                Filter::Not(_) | Filter::Empty | Filter::Pred(_) => None,
            }
        }
        find(self).unwrap_or(KindFilter::Any)
    }

    /// Every positive text predicate in the tree, in query order. Used by the
    /// ranking step to score a hit without re-walking the expression.
    pub fn text_predicates(&self) -> Vec<&TextPredicate> {
        let mut out = Vec::new();
        fn walk<'a>(filter: &'a Filter, negative: bool, out: &mut Vec<&'a TextPredicate>) {
            match filter {
                Filter::Pred(Predicate::Text(text)) if !negative => out.push(text),
                Filter::Not(inner) => walk(inner, !negative, out),
                Filter::All(items) | Filter::Any(items) => {
                    for item in items {
                        walk(item, negative, out);
                    }
                }
                Filter::Pred(_) | Filter::Empty => {}
            }
        }
        walk(self, false, &mut out);
        out
    }
}

/// One record's searchable text, borrowed from the index arena.
pub struct Haystack<'a> {
    /// The file name.
    pub name: &'a [u8],
    /// The full path, materialised on demand and only when a filter needs it.
    pub path: Option<&'a str>,
}

impl<'a> Haystack<'a> {
    pub fn new(name: &'a [u8]) -> Self {
        Self { name, path: None }
    }

    pub fn with_path(name: &'a [u8], path: &'a str) -> Self {
        Self {
            name,
            path: Some(path),
        }
    }

    fn text(&self, scope: Scope) -> &[u8] {
        match scope {
            Scope::Name => self.name,
            Scope::Path => self.path.map(str::as_bytes).unwrap_or(self.name),
        }
    }
}

/// Evaluate a compiled filter against one record.
///
/// `size`/`mtime` are `None` when the enumerator did not report the metadata;
/// those predicates then fail (report §3.3 keeps metadata optional per index
/// configuration, and a filter that cannot be evaluated must not match).
pub fn evaluate(
    filter: &Filter,
    haystack: &Haystack<'_>,
    size: Option<u64>,
    mtime: Option<u64>,
    is_dir: bool,
) -> bool {
    match filter {
        Filter::Empty => true,
        Filter::Not(inner) => !evaluate(inner, haystack, size, mtime, is_dir),
        Filter::All(items) => items
            .iter()
            .all(|item| evaluate(item, haystack, size, mtime, is_dir)),
        Filter::Any(items) => items
            .iter()
            .any(|item| evaluate(item, haystack, size, mtime, is_dir)),
        Filter::Pred(predicate) => match predicate {
            Predicate::Text(text) => text.matches(haystack.text(text.scope)),
            Predicate::Extension(extension) => {
                let name = haystack.name;
                name.len() > extension.len() + 1
                    && name[name.len() - extension.len() - 1] == b'.'
                    && name[name.len() - extension.len()..]
                        .eq_ignore_ascii_case(extension.as_bytes())
            }
            Predicate::Size(filter) => {
                size.is_some_and(|value| filter.op.apply(value, filter.bytes))
            }
            Predicate::Modified(filter) => {
                mtime.is_some_and(|value| filter.op.apply(value, filter.mtime))
            }
            Predicate::Kind(KindFilter::Any) => true,
            Predicate::Kind(KindFilter::FilesOnly) => !is_dir,
            Predicate::Kind(KindFilter::FoldersOnly) => is_dir,
        },
    }
}

/// Parse a query string into a [`Filter`]. Never fails: an unusable modifier
/// degrades to a literal text term, which keeps a search box forgiving.
pub fn parse(query: &str) -> Filter {
    let mut builder = Builder::default();
    let mut group: Option<Vec<Filter>> = None;
    let mut group_negated = false;
    for token in tokenize(query) {
        match token {
            Token::OpenGroup { negated } => {
                // Nested groups are not worth the ambiguity: start a fresh one.
                if let Some(previous) = group.take() {
                    builder.push(Filter::Any(previous));
                }
                group = Some(Vec::new());
                group_negated = negated;
            }
            Token::CloseGroup => {
                if let Some(items) = group.take() {
                    let expression = Filter::Any(items);
                    builder.push(if group_negated {
                        Filter::Not(Box::new(expression))
                    } else {
                        expression
                    });
                    group_negated = false;
                }
            }
            Token::Modifier { name, value } => {
                let scope = builder.apply_modifier(&name, value.as_deref());
                if let Some(filter) = scope {
                    match group.as_mut() {
                        Some(items) => items.push(filter),
                        None => builder.push(filter),
                    }
                }
            }
            Token::Term { text, negated } => {
                if let Some(filter) = builder.term(&text, negated) {
                    match group.as_mut() {
                        Some(items) => items.push(filter),
                        None => builder.push(filter),
                    }
                }
            }
        }
    }
    if let Some(items) = group.take() {
        builder.push(Filter::Any(items));
    }
    builder.finish()
}

#[derive(Default)]
struct Builder {
    positive: Vec<Filter>,
    negative: Vec<Filter>,
    scope: Scope,
    mode: MatchMode,
    case: CaseMode,
    kind: Option<KindFilter>,
    saw_kind: bool,
}

impl Builder {
    /// Append a filter, routing negations into their own list so the final tree
    /// is `All(positive.., Not(negative)..)`.
    fn push(&mut self, filter: Filter) {
        match filter {
            Filter::Not(inner) => self.negative.push(*inner),
            other => self.positive.push(other),
        }
    }

    fn finish(self) -> Filter {
        let mut items = self.positive;
        items.extend(
            self.negative
                .into_iter()
                .map(|inner| Filter::Not(Box::new(inner))),
        );
        match items.len() {
            0 => {
                // Nothing typed but a state modifier was set, which is a valid
                // "list everything of this kind" query (`folder:`, `type:file`).
                match self.kind {
                    Some(kind) if kind != KindFilter::Any => Filter::Pred(Predicate::Kind(kind)),
                    _ => Filter::Empty,
                }
            }
            1 => items.pop().expect("length checked"),
            _ => Filter::All(items),
        }
    }

    /// Apply a `modifier:value` token.
    ///
    /// Returns a filter when the token itself constrains the query, and `None`
    /// for a bare state modifier (which scopes the terms that follow). Exported
    /// shapes follow the report's examples: `case:` / `path:` / `regex:` act on
    /// the rest of the query, while `wildcards:*.pdf` and `wholeword:report`
    /// carry their own pattern.
    fn apply_modifier(&mut self, name: &str, value: Option<&str>) -> Option<Filter> {
        let value = value.unwrap_or("");
        // A modifier with a value still switches the mode on, so the rest of a
        // mixed query (`regex:^a path:src`) keeps a consistent reading.
        match name {
            "case" => self.case = CaseMode::Sensitive,
            "nocase" => self.case = CaseMode::Folded,
            "path" => self.scope = Scope::Path,
            "name" => self.scope = Scope::Name,
            "regex" => self.mode = MatchMode::Regex,
            "wildcards" | "wildcard" => self.mode = MatchMode::Wildcards,
            "wholeword" | "ww" => self.mode = MatchMode::WholeWord,
            _ => {}
        }
        // A text-valued form (`case:report`, `wildcards:*.pdf`, `regex:^a.*b$`)
        // additionally *is* the term, using the state set above.
        if value.is_empty() {
            return match name {
                "type" | "kind" => self.kind_from_value("folder"),
                "folder" | "folders" => {
                    self.saw_kind = true;
                    Some(Filter::Pred(Predicate::Kind(KindFilter::FoldersOnly)))
                }
                _ => None,
            };
        }
        match name {
            "case" | "nocase" | "path" | "name" | "regex" | "wildcards" | "wildcard"
            | "wholeword" | "ww" => Some(text_predicate(value, self.scope, self.mode, self.case)),
            "ext" => {
                let extension = value.trim_start_matches('.').to_ascii_lowercase();
                (!extension.is_empty()).then(|| Filter::Pred(Predicate::Extension(extension)))
            }
            "size" => parse_size(value).map(|filter| Filter::Pred(Predicate::Size(filter))),
            "dm" | "date" | "modified" => {
                parse_date(value).map(|filter| Filter::Pred(Predicate::Modified(filter)))
            }
            "type" | "kind" => self.kind_from_value(value),
            "folder" | "folders" => {
                // `folder:name` searches directories for a text term.
                self.saw_kind = true;
                Some(Filter::All(vec![
                    Filter::Pred(Predicate::Kind(KindFilter::FoldersOnly)),
                    text_predicate(value, self.scope, self.mode, self.case),
                ]))
            }
            _ => None,
        }
    }

    fn kind_from_value(&mut self, value: &str) -> Option<Filter> {
        match value.to_ascii_lowercase().as_str() {
            "file" | "files" => {
                self.saw_kind = true;
                Some(Filter::Pred(Predicate::Kind(KindFilter::FilesOnly)))
            }
            "folder" | "folders" | "dir" | "dirs" => {
                self.saw_kind = true;
                Some(Filter::Pred(Predicate::Kind(KindFilter::FoldersOnly)))
            }
            _ => None,
        }
    }

    /// Build the text predicate for one term, honouring the current state
    /// modifiers, and wrap it in a negation when asked.
    fn term(&mut self, text: &str, negated: bool) -> Option<Filter> {
        if text.is_empty() {
            return None;
        }
        // A term that looks like a path searches the full path without needing
        // an explicit `path:` — typing `c:\users` must not be read as a file
        // name that happens to contain backslashes.
        let scope = if self.scope == Scope::Name && looks_like_path(text) {
            Scope::Path
        } else {
            self.scope
        };
        let predicate = text_predicate(text, scope, self.mode, self.case);
        Some(if negated {
            Filter::Not(Box::new(predicate))
        } else {
            predicate
        })
    }
}

/// Build a text predicate, compiling the regex up front when the mode needs one.
///
/// A regex that fails to compile is kept with `regex: None`, which makes the
/// predicate match nothing rather than everything — a half-typed pattern must
/// not turn into "show me the whole disk".
fn text_predicate(text: &str, scope: Scope, mode: MatchMode, case: CaseMode) -> Filter {
    let regex = (mode == MatchMode::Regex)
        .then(|| {
            let source = match case {
                CaseMode::Folded => format!("(?i){text}"),
                CaseMode::Sensitive => text.to_owned(),
            };
            Regex::new(&source).ok()
        })
        .flatten();
    Filter::Pred(Predicate::Text(TextPredicate {
        text: text.to_owned(),
        mode,
        scope,
        case,
        regex,
    }))
}

/// A lexed token.
#[derive(Debug, PartialEq, Eq)]
enum Token {
    OpenGroup { negated: bool },
    CloseGroup,
    Modifier { name: String, value: Option<String> },
    Term { text: String, negated: bool },
}

/// Split a query into tokens, honouring quotes and `< >` groups.
fn tokenize(query: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars = query.chars().peekable();
    let mut word = String::new();
    let mut in_quotes = false;

    macro_rules! flush_word {
        () => {
            if !word.is_empty() {
                tokens.push(classify(std::mem::take(&mut word)));
            }
        };
    }

    for character in chars {
        match character {
            '"' => {
                in_quotes = !in_quotes;
                if !in_quotes {
                    flush_word!();
                }
            }
            // `size:>10M` / `dm:<2024-01-01` use the comparison operators as
            // part of a modifier value, so they must not open or close a group.
            '<' | '>' if !in_quotes && is_operator_prefix(&word) => word.push(character),
            '<' if !in_quotes => {
                flush_word!();
                // `!<a b>` excludes both alternatives: the negation belongs to
                // the group, not to its first member. The tokenizer keeps the
                // `!` as its own word for that reason (see `classify`), so it is
                // still on the token list here.
                let negated = matches!(tokens.last(), Some(Token::Term { text, negated: true }) if text == "!");
                if negated {
                    tokens.pop();
                }
                tokens.push(Token::OpenGroup { negated });
            }
            '>' if !in_quotes => {
                flush_word!();
                tokens.push(Token::CloseGroup);
            }
            c if c.is_whitespace() && !in_quotes => {
                flush_word!();
            }
            c => word.push(c),
        }
    }
    flush_word!();
    tokens
}

/// Whether the word accumulated so far is a modifier that takes a comparison
/// operator as its value (`size:`, `dm:`, `date:`, `modified:`).
fn is_operator_prefix(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "size:" | "dm:" | "date:" | "modified:"
    )
}

/// Turn a raw word into a modifier or a term.
fn classify(word: String) -> Token {
    // A lone `!` is kept as a token: the group rule (`!<a b>`) needs to see it.
    if word == "!" {
        return Token::Term {
            text: word,
            negated: true,
        };
    }
    let (negated, body) = if let Some(rest) = word.strip_prefix('!') {
        (true, rest.to_string())
    } else if let Some(rest) = word.strip_prefix('-') {
        // A leading `-` negates a word (Everything's exclusion syntax), but a
        // digit or a bare `-` stays literal so `-5`/`--flag` remain searches.
        if rest.is_empty()
            || rest.starts_with('-')
            || rest.starts_with(|c: char| c.is_ascii_digit())
        {
            (false, word)
        } else {
            (true, rest.to_string())
        }
    } else {
        (false, word)
    };
    if !negated {
        if let Some((name, value)) = split_modifier(&body) {
            return Token::Modifier { name, value };
        }
    }
    Token::Term {
        text: body,
        negated,
    }
}

/// Recognised modifier names. A word only counts as a modifier when its prefix
/// is in this list, which is what keeps `C:\Users` from parsing as a modifier.
const MODIFIERS: [&str; 17] = [
    "case",
    "nocase",
    "path",
    "name",
    "regex",
    "wildcards",
    "wildcard",
    "wholeword",
    "ww",
    "ext",
    "size",
    "dm",
    "date",
    "modified",
    "type",
    "kind",
    "folder",
];

/// Whether a term looks like a path rather than a file name: it contains a
/// separator, or it is a bare drive (`c:`). Used to give such terms the full
/// haystack automatically, the way a file manager's search box behaves.
fn looks_like_path(text: &str) -> bool {
    if text.contains('\\') || text.contains('/') {
        return true;
    }
    let bytes = text.as_bytes();
    bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// Placeholder kept so the modifier count stays explicit when the list grows.
const _MODIFIER_COUNT_CHECK: () = {
    assert!(MODIFIERS.len() == 17);
};

/// Split `modifier:value`, accepting only known modifier names.
fn split_modifier(word: &str) -> Option<(String, Option<String>)> {
    let (name, value) = word.split_once(':')?;
    let name = name.to_ascii_lowercase();
    if !MODIFIERS.contains(&name.as_str()) {
        return None;
    }
    Some((name, (!value.is_empty()).then(|| value.to_string())))
}

/// `size:` value: `>10M`, `<=1.5G`, `512K`, `1024`.
fn parse_size(value: &str) -> Option<SizeFilter> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (op, rest) = split_operator(value);
    let (number, multiplier) = match rest.chars().last() {
        Some(unit) if unit.is_ascii_alphabetic() => {
            let multiplier = match unit.to_ascii_lowercase() {
                'k' => 1024u64,
                'm' => 1024 * 1024,
                'g' => 1024 * 1024 * 1024,
                't' => 1024u64.pow(4),
                'b' => 1,
                _ => return None,
            };
            (&rest[..rest.len() - unit.len_utf8()], multiplier)
        }
        _ => (rest, 1),
    };
    let number: f64 = number.trim().parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let bytes = (number * multiplier as f64) as u64;
    Some(SizeFilter { op, bytes })
}

/// `dm:` value: `today`, `week`, `>2024-01-01`, `7d`, `12h`.
///
/// The threshold is resolved against `now` (UNIX seconds) so a query means the
/// same thing for one scan; relative units are `d`/`w`/`h`/`mo`/`y`.
fn parse_date(value: &str) -> Option<DateFilter> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let today_start = unix_midnight(now);

    let (op, rest) = split_operator(value);
    let lowered = rest.to_ascii_lowercase();
    let threshold_unix = match lowered.as_str() {
        "today" => Some(today_start),
        "yesterday" => Some(today_start - 86_400),
        "week" => Some(today_start - 7 * 86_400),
        "month" => Some(today_start - 30 * 86_400),
        "year" => Some(today_start - 365 * 86_400),
        _ => {
            if let Some(absolute) = parse_absolute_date(&lowered) {
                Some(absolute)
            } else {
                parse_relative_duration(&lowered).map(|seconds| now - seconds)
            }
        }
    }?;
    Some(DateFilter {
        op,
        mtime: super::db::unix_to_mtime(threshold_unix),
    })
}

/// `>`, `<`, `>=`, `<=` prefix, defaulting to "greater or equal" (the natural
/// reading of `size:10M` / `dm:today`).
fn split_operator(value: &str) -> (Compare, &str) {
    if let Some(rest) = value.strip_prefix(">=") {
        (Compare::GreaterOrEqual, rest)
    } else if let Some(rest) = value.strip_prefix("<=") {
        (Compare::LessOrEqual, rest)
    } else if let Some(rest) = value.strip_prefix('>') {
        (Compare::Greater, rest)
    } else if let Some(rest) = value.strip_prefix('<') {
        (Compare::Less, rest)
    } else {
        (Compare::GreaterOrEqual, value)
    }
}

/// `2024-01-01` / `2024/01/01` → UNIX seconds at midnight UTC.
fn parse_absolute_date(value: &str) -> Option<i64> {
    let normalized = value.replace('/', "-");
    let mut parts = normalized.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(1601..=9999).contains(&year) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400)
}

/// `7d`, `12h`, `2w`, `6mo`, `1y` → a duration in seconds.
fn parse_relative_duration(value: &str) -> Option<i64> {
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    let unit = &value[digits.len()..];
    let amount: i64 = digits.parse().ok()?;
    let unit_seconds = match unit {
        "h" | "hr" | "hour" | "hours" => 3_600,
        "d" | "day" | "days" => 86_400,
        "w" | "week" | "weeks" => 7 * 86_400,
        "mo" | "month" | "months" => 30 * 86_400,
        "y" | "year" | "years" => 365 * 86_400,
        _ => return None,
    };
    Some(amount * unit_seconds)
}

/// Midnight UTC of the day containing `unix`.
fn unix_midnight(unix: i64) -> i64 {
    unix.div_euclid(86_400) * 86_400
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`), so no calendar crate is needed for `dm:2024-01-01`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_shift = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shift + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Folded (case-insensitive ASCII) substring search.
///
/// The haystack comes straight out of the index arena. Folding a byte is
/// branch-free: `b | 0x20` maps `A-Z` onto `a-z` and leaves every other byte
/// that matters unchanged (`[` `\` `]` `^` `_` map onto punctuation, which can
/// only ever produce a false positive on a name containing those bytes AND the
/// query being their folded twin — accepted, as in the original's byte-fold
/// table).
fn substring_match(haystack: &[u8], needle: &[u8], case: CaseMode) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    match case {
        CaseMode::Sensitive => haystack
            .windows(needle.len())
            .any(|window| window == needle),
        CaseMode::Folded => {
            // Fast reject on the first byte before comparing windows: the vast
            // majority of records fail here, which is where the scan spends its
            // time.
            let first = fold(needle[0]);
            let mut start = 0;
            while let Some(position) = haystack[start..]
                .iter()
                .position(|byte| fold(*byte) == first)
            {
                let at = start + position;
                if at + needle.len() > haystack.len() {
                    return false;
                }
                if haystack[at..at + needle.len()]
                    .iter()
                    .zip(needle)
                    .all(|(h, n)| fold(*h) == fold(*n))
                {
                    return true;
                }
                start = at + 1;
            }
            false
        }
    }
}

/// Case-insensitive ASCII fold (`A-Z` → `a-z`, everything else unchanged).
#[inline]
fn fold(byte: u8) -> u8 {
    byte | 0x20
}

/// Cheap "does this needle occur in this buffer" test for the ranking step,
/// so scoring does not have to build a [`TextPredicate`].
pub(crate) fn substring_probe(haystack: &[u8], needle: &[u8]) -> bool {
    substring_match(haystack, needle, CaseMode::Folded)
}

/// How well a record matches a query, as an ordered tier.
///
/// The launcher merges several result kinds into one list (applications, plugin
/// commands, indexed files) and the drop-down only shows a handful of rows. If
/// each kind is appended in its own block, a file whose *name is exactly the
/// query* can be pushed out of sight by applications that merely contain the
/// query's letters — which reads as "it only matches from the first letter".
/// Ranking every row by how well it actually matches fixes that without
/// favouring one kind over another.
///
/// The ordering is the contract, so the variants are declared best first and the
/// derived `Ord` pins it (see `tiers_are_ordered_best_first`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchTier {
    /// The name *is* the query.
    ExactName,
    /// The name starts with the query.
    NamePrefix,
    /// The query appears somewhere inside the name.
    NameSubstring,
    /// Only a fuzzy subsequence or the full path matches.
    Weak,
}

/// Classify a match between the query text and a record's name, with its full
/// path supplied when the caller has one.
///
/// `needle` is the plain text of the query's first positive term. Comparison is
/// case-insensitive, matching what a plain term means at match time.
pub fn match_tier(needle: &str, name: &str, path: Option<&str>) -> MatchTier {
    let needle = needle.trim();
    if needle.is_empty() {
        return MatchTier::Weak;
    }
    let folded_needle = needle.to_lowercase();
    let folded_name = name.to_lowercase();
    if folded_name == folded_needle {
        return MatchTier::ExactName;
    }
    // `notes` against `notes.md` is an exact hit on the stem, which is what the
    // user typed: a launcher rarely needs the extension spelled out. The needle
    // must not itself contain a dot, so `archive.tar` is not read as exact for
    // `archive.tar.gz`.
    if !folded_needle.contains('.') {
        if let Some(stem) = folded_name.rsplit_once('.').map(|(stem, _)| stem) {
            if stem == folded_needle {
                return MatchTier::ExactName;
            }
        }
    }
    if folded_name.starts_with(&folded_needle) {
        return MatchTier::NamePrefix;
    }
    if substring_match(
        folded_name.as_bytes(),
        folded_needle.as_bytes(),
        CaseMode::Folded,
    ) {
        return MatchTier::NameSubstring;
    }
    // A path hit ranks no better than a fuzzy one: `path:` queries still reach
    // it, but a name hit is what the user usually typed.
    if let Some(path) = path {
        if substring_match(
            path.to_lowercase().as_bytes(),
            folded_needle.as_bytes(),
            CaseMode::Folded,
        ) {
            return MatchTier::Weak;
        }
    }
    MatchTier::Weak
}

/// Whether `byte` is a word character for `wholeword:` (ASCII only; any
/// non-ASCII byte counts as a word character, matching how CJK runs behave).
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii()
}

/// `wholeword:` variant: the substring must not be glued to a word character.
fn whole_word_match(haystack: &[u8], needle: &[u8], case: CaseMode) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        let found = match case {
            CaseMode::Sensitive => haystack[start..]
                .windows(needle.len())
                .position(|window| window == needle)
                .map(|offset| start + offset),
            CaseMode::Folded => {
                let first = fold(needle[0]);
                haystack[start..]
                    .iter()
                    .position(|byte| fold(*byte) == first)
                    .map(|offset| start + offset)
                    .filter(|at| {
                        at + needle.len() <= haystack.len()
                            && haystack[*at..*at + needle.len()]
                                .iter()
                                .zip(needle)
                                .all(|(h, n)| fold(*h) == fold(*n))
                    })
            }
        };
        let Some(at) = found else {
            return false;
        };
        let before_ok = at == 0 || !is_word_byte(haystack[at - 1]);
        let after = at + needle.len();
        let after_ok = after >= haystack.len() || !is_word_byte(haystack[after]);
        if before_ok && after_ok {
            return true;
        }
        start = at + 1;
    }
    false
}

/// `wildcards:` matching with `*` (any run of characters) and `?` (one
/// character). Backtracking is iterative and linear per star, so a pathological
/// pattern cannot blow the stack.
fn wildcard_match(haystack: &[u8], pattern: &[u8], case: CaseMode) -> bool {
    let equal = |h: u8, p: u8| match case {
        CaseMode::Sensitive => h == p,
        CaseMode::Folded => fold(h) == fold(p),
    };
    let (mut h, mut p) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while h < haystack.len() {
        if p < pattern.len() && (pattern[p] == b'?' || equal(haystack[h], pattern[p])) {
            h += 1;
            p += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, h));
            p += 1;
        } else if let Some((star_p, star_h)) = star {
            p = star_p + 1;
            h = star_h + 1;
            star = Some((star_p, star_h + 1));
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_index::db::unix_to_mtime;

    /// Evaluate a query against one name (no path, no metadata).
    fn matches(query: &str, name: &str) -> bool {
        let filter = parse(query);
        let haystack = Haystack::new(name.as_bytes());
        evaluate(&filter, &haystack, None, None, false)
    }

    /// Evaluate a query against one name + full path.
    fn matches_path(query: &str, name: &str, path: &str) -> bool {
        let filter = parse(query);
        let haystack = Haystack::with_path(name.as_bytes(), path);
        evaluate(&filter, &haystack, None, None, false)
    }

    #[test]
    fn plain_terms_are_and_ed_and_folded() {
        assert!(matches("report", "Annual-Report.pdf"));
        assert!(matches("annual report", "Annual-Report.pdf"));
        assert!(!matches("annual budget", "Annual-Report.pdf"));
        assert!(matches("REPORT", "annual-report.pdf"));
    }

    #[test]
    fn empty_query_matches_everything() {
        let filter = parse("   ");
        assert!(filter.is_always());
        assert!(matches("", "anything.txt"));
    }

    #[test]
    fn quoted_terms_keep_their_spaces() {
        assert!(matches("\"annual report\"", "annual report.pdf"));
        assert!(!matches("\"annualreport\"", "annual report.pdf"));
    }

    #[test]
    fn negation_removes_matches() {
        assert!(matches("report !draft", "annual-report.pdf"));
        assert!(!matches("report !draft", "annual-report-draft.pdf"));
        assert!(!matches("report -draft", "annual-report-draft.pdf"));
        // A leading dash on a number stays literal.
        assert!(matches("-5", "log-5.txt"));
    }

    #[test]
    fn groups_are_or_ed() {
        assert!(matches("budget <report invoice>", "budget-report.pdf"));
        assert!(matches("budget <report invoice>", "budget-invoice.pdf"));
        assert!(matches(
            "budget <report invoice>",
            "budget-report-invoice.pdf"
        ));
        assert!(!matches("budget <report invoice>", "budget-notes.pdf"));
        assert!(!matches("budget <report invoice>", "annual-report.pdf"));
        assert!(!matches("report <a b>", "report-z.pdf"));
    }

    #[test]
    fn negated_groups_exclude_both_alternatives() {
        assert!(!matches("!<draft tmp>", "report-draft.pdf"));
        assert!(!matches("!<draft tmp>", "report.tmp"));
        assert!(matches("!<draft tmp>", "report.pdf"));
    }

    #[test]
    fn case_modifier_switches_to_exact_comparison() {
        assert!(!matches("case:report", "Report.pdf"));
        assert!(matches("case:report", "report.pdf"));
        // Bare `case:` applies to every following term.
        assert!(!matches("case: report budget", "report-Budget.pdf"));
    }

    #[test]
    fn path_modifier_searches_the_full_path() {
        assert!(matches_path(
            "path:users",
            "notes.txt",
            "C:\\Users\\a\\notes.txt"
        ));
        assert!(!matches_path(
            "users",
            "notes.txt",
            "C:\\Users\\a\\notes.txt"
        ));
        // A bare `path:` scopes the rest of the query.
        assert!(matches_path(
            "path: users notes",
            "notes.txt",
            "C:\\Users\\a\\notes.txt"
        ));
    }

    #[test]
    fn wildcards_and_wholeword_modes_change_matching() {
        assert!(matches("wildcards:rep*.pdf", "report.pdf"));
        assert!(!matches("wildcards:rep*.pdf", "report.txt"));
        assert!(matches("wildcards:?eport.pdf", "report.pdf"));
        assert!(matches("wholeword:report", "annual report.pdf"));
        assert!(!matches("wholeword:report", "reporting.pdf"));
        assert!(
            matches("report", "reporting.pdf"),
            "plain mode is substring"
        );
    }

    #[test]
    fn regex_terms_use_the_regex_crate() {
        assert!(matches(r"regex:^rep.*\.pdf$", "report.pdf"));
        assert!(!matches(r"regex:^rep.*\.pdf$", "x-report.pdf"));
        assert!(
            !matches("regex:(", "anything"),
            "an uncompilable pattern matches nothing"
        );
        // An uncompilable pattern matches nothing instead of everything.
        assert!(!matches("regex:(", "anything"));
    }

    #[test]
    fn extension_filter_matches_the_suffix() {
        assert!(matches("ext:pdf", "report.PDF"));
        assert!(matches("ext:.pdf", "report.pdf"));
        assert!(!matches("ext:pdf", "pdfreport.txt"));
        assert!(!matches("ext:pdf", "pdf"));
    }

    #[test]
    fn size_filter_understands_units_and_operators() {
        let filter = parse("size:>10M");
        let haystack = Haystack::new(b"big.bin");
        assert!(evaluate(&filter, &haystack, Some(11 << 20), None, false));
        assert!(!evaluate(&filter, &haystack, Some(9 << 20), None, false));
        // Unknown size cannot satisfy a size filter.
        assert!(!evaluate(&filter, &haystack, None, None, false));

        let small = parse("size:<1K");
        assert!(evaluate(&small, &haystack, Some(512), None, false));
        let exact = parse("size:1G");
        assert!(evaluate(&exact, &haystack, Some(1 << 30), None, false));
    }

    #[test]
    fn malformed_size_filters_stay_literal_terms() {
        // Unparseable modifiers degrade to text so the search box never breaks.
        assert!(matches("size:banana", "size:banana.txt"));
    }

    #[test]
    fn date_filter_accepts_absolute_and_relative_forms() {
        let absolute = parse("dm:>2024-01-01");
        let haystack = Haystack::new(b"x.txt");
        let after = unix_to_mtime(1_704_067_200 + 86_400);
        let before = unix_to_mtime(1_704_067_200 - 86_400);
        assert!(evaluate(&absolute, &haystack, None, Some(after), false));
        assert!(!evaluate(&absolute, &haystack, None, Some(before), false));

        // `dm:today` is relative to the clock, so just assert it is satisfied
        // by a file modified now.
        let today = parse("dm:today");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(evaluate(
            &today,
            &haystack,
            None,
            Some(unix_to_mtime(now as i64)),
            false
        ));
        assert!(evaluate(
            &parse("dm:1d"),
            &haystack,
            None,
            Some(unix_to_mtime(now as i64)),
            false
        ));
    }

    #[test]
    fn kind_filters_select_files_or_folders() {
        let files = parse("type:file report");
        let folders = parse("type:folder report");
        let haystack = Haystack::new(b"report");
        assert!(evaluate(&files, &haystack, None, None, false));
        assert!(!evaluate(&files, &haystack, None, None, true));
        assert!(evaluate(&folders, &haystack, None, None, true));
        assert!(!evaluate(&folders, &haystack, None, None, false));
        // `folder:` alone lists directories.
        let bare = parse("folder:");
        assert_eq!(bare.kind(), KindFilter::FoldersOnly);
        assert!(evaluate(&bare, &haystack, None, None, true));
    }

    #[test]
    fn windows_paths_are_not_mistaken_for_modifiers() {
        // `C:` is not a modifier, so the whole token stays a search term.
        assert!(matches_path(
            r"c:\users",
            "notes.txt",
            r"C:\Users\a\notes.txt"
        ));
        assert!(matches("http://x", "http://x.txt"));
        assert!(matches("a:b", "a:b.txt"));
    }

    #[test]
    fn unknown_modifiers_are_literal_text() {
        assert!(matches("colour:red", "colour:red.txt"));
        assert!(!matches("colour:red", "color-red.txt"));
    }

    #[test]
    fn needs_path_reports_when_parent_chains_are_required() {
        assert!(!parse("report").needs_path());
        assert!(parse("path:users").needs_path());
        assert!(parse("report <path:users budget>").needs_path());
        assert!(!parse("report !budget").needs_path());
    }

    #[test]
    fn text_predicates_are_listed_for_ranking() {
        let filter = parse("report <budget invoice> !draft");
        let texts: Vec<&str> = filter
            .text_predicates()
            .iter()
            .map(|predicate| predicate.text.as_str())
            .collect();
        assert_eq!(texts, vec!["report", "budget", "invoice"]);
    }

    #[test]
    fn days_from_civil_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 1, 1), 19_723);
        assert_eq!(parse_absolute_date("2024-01-01"), Some(19_723 * 86_400));
        assert_eq!(
            parse_absolute_date("2024/2/29"),
            Some((19_723 + 59) * 86_400)
        );
        assert_eq!(parse_absolute_date("2024-13-01"), None);
        assert_eq!(parse_absolute_date("2024-01-32"), None);
    }

    #[test]
    fn is_never_detects_unsatisfiable_queries() {
        let filter = parse("report !report");
        assert!(filter.is_never(), "p and not p can never match");
        assert!(!parse("report").is_never());
    }

    #[test]
    fn wildcard_matching_handles_stars_and_questions() {
        assert!(wildcard_match(b"report.pdf", b"*.pdf", CaseMode::Folded));
        assert!(wildcard_match(b"report.pdf", b"r*t.pdf", CaseMode::Folded));
        assert!(!wildcard_match(b"report.pdf", b"*.txt", CaseMode::Folded));
        assert!(wildcard_match(b"a", b"*", CaseMode::Folded));
        assert!(wildcard_match(b"abc", b"a?c", CaseMode::Folded));
        assert!(!wildcard_match(b"ac", b"a?c", CaseMode::Folded));
        assert!(wildcard_match(b"REPORT", b"report", CaseMode::Folded));
        assert!(!wildcard_match(b"REPORT", b"report", CaseMode::Sensitive));
    }
}
