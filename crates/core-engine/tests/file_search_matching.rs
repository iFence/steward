//! Does a plain term match anywhere in a name, and does the matching row reach
//! the top of the list? The report's own `SUBSTRING_FOLDED` opcode is a
//! anywhere-in-the-name match, so this pins that behaviour at the level the
//! launcher actually uses: parse -> search -> rank -> truncate.

use steward_core_engine::file_index::{
    match_tier, parse_filter, search_filtered, EntryInfo, FileDbBuilder, KindFilter, MatchTier,
    SearchOptions,
};

/// A small tree with names that exercise each match position.
fn index() -> steward_core_engine::FileDb {
    let mut builder = FileDbBuilder::new();
    let root = builder.add_root("D:\\", 1);
    let dir = builder.add_child(root, &EntryInfo::dir("projects"));
    let deep = builder.add_child(dir, &EntryInfo::dir("archive"));
    for name in [
        "report.pdf",
        "annual-report.pdf",
        "myreport.txt",
        "reports-2024.xlsx",
        "notes.md",
    ] {
        builder.add_child(dir, &EntryInfo::file(name));
    }
    builder.add_child(deep, &EntryInfo::file("old-report.pdf"));
    builder.finalize()
}

fn names(query: &str) -> Vec<String> {
    let db = index();
    let options = SearchOptions {
        limit: 12,
        kind: KindFilter::FilesOnly,
        sort_by_recency: false,
    };
    let outcome = search_filtered(&db, &parse_filter(query), &options, None);
    outcome.hits.iter().map(|hit| hit.name.clone()).collect()
}

#[test]
fn a_term_matches_anywhere_in_the_name() {
    // Prefix, middle and suffix positions all match; the ordering among them is
    // the ranking's business (see `tiers_are_ordered_best_first`).
    let hits = names("report");
    assert_eq!(
        hits.len(),
        5,
        "every name containing the term must match, got {hits:?}"
    );
    for expected in [
        "report.pdf",
        "reports-2024.xlsx",
        "myreport.txt",
        "annual-report.pdf",
        "old-report.pdf",
    ] {
        assert!(
            hits.contains(&expected.to_string()),
            "{expected} must match, got {hits:?}"
        );
    }
    // A prefix hit outranks a mid-name hit, which outranks a path-only one.
    assert_eq!(hits.first().map(String::as_str), Some("report.pdf"));
}

#[test]
fn tiers_are_ordered_best_first() {
    // A file whose name is exactly the query must be able to outrank an
    // application that merely fuzzy-contains the query's letters. That is only
    // possible if the tiers are ordered, so pin the order itself.
    assert!(MatchTier::ExactName < MatchTier::NamePrefix);
    assert!(MatchTier::NamePrefix < MatchTier::NameSubstring);
    assert!(MatchTier::NameSubstring < MatchTier::Weak);

    assert_eq!(
        match_tier("notes", "notes.md", Some("D:\\notes.md")),
        MatchTier::ExactName
    );
    assert_eq!(
        match_tier("notes", "notes-2024.md", Some("D:\\notes-2024.md")),
        MatchTier::NamePrefix
    );
    assert_eq!(
        match_tier("notes", "mynotes.md", Some("D:\\mynotes.md")),
        MatchTier::NameSubstring
    );
    // Only the parent directory matches: reachable with `path:`, but weak.
    assert_eq!(
        match_tier("archive", "old.pdf", Some("D:\\archive\\old.pdf")),
        MatchTier::Weak
    );
    // No match at all is still `Weak`, never better.
    assert_eq!(
        match_tier("zzz", "old.pdf", Some("D:\\archive\\old.pdf")),
        MatchTier::Weak
    );
    // Case-insensitive, like a plain query term.
    assert_eq!(match_tier("NOTES", "notes.md", None), MatchTier::ExactName);
    // A blank needle must not claim the best tier.
    assert_eq!(match_tier("   ", "notes.md", None), MatchTier::Weak);
}

#[test]
fn a_mid_name_fragment_matches() {
    // "eport" only ever appears after the first character.
    let hits = names("eport");
    assert!(
        hits.contains(&"report.pdf".to_string()),
        "mid-name fragment must match, got {hits:?}"
    );
    assert!(hits.contains(&"myreport.txt".to_string()), "got {hits:?}");
}

#[test]
fn a_suffix_fragment_matches() {
    let hits = names("report.pdf");
    assert!(hits.contains(&"report.pdf".to_string()), "got {hits:?}");
    assert!(
        hits.contains(&"annual-report.pdf".to_string()),
        "got {hits:?}"
    );
}

#[test]
fn a_match_inside_a_parent_directory_is_reachable_with_path() {
    // Without `path:` the parent directory is not searched, so "projects" only
    // matches through `path:` — and it must not match nothing.
    assert!(
        names("projects").is_empty(),
        "a directory name alone is not a name match"
    );
    let hits = names("path:projects report");
    assert!(hits.contains(&"report.pdf".to_string()), "got {hits:?}");
}

#[test]
fn ranking_puts_the_closest_name_first() {
    // An exact name beats a prefix beats a mid-name hit beats a path-only hit.
    let hits = names("notes");
    assert_eq!(hits.first().map(String::as_str), Some("notes.md"));
}

/// The launcher ranks every row by match quality, not by result kind. This is the
/// regression that made search look like it "only matches from the first
/// letter": the drop-down shows eight rows, and files used to be appended after
/// the applications, so eight fuzzy application hits hid every file.
#[test]
fn a_file_name_match_outranks_a_weaker_application_match() {
    // A file whose stem is exactly the query.
    let file = match_tier("report", "report.pdf", Some("D:\\projects\\report.pdf"));
    // An application that merely contains the query's letters in order, the way
    // nucleo's fuzzy match would accept it.
    let app = match_tier(
        "report",
        "Repository Tools",
        Some("C:\\apps\\repo-tools.exe"),
    );
    assert_eq!(file, MatchTier::ExactName);
    assert_eq!(app, MatchTier::Weak);
    assert!(
        file < app,
        "an exactly-named file must be able to lead the list over a fuzzy app hit"
    );

    // And a mid-name file hit still beats a weak one.
    let mid = match_tier("report", "myreport.txt", Some("D:\\myreport.txt"));
    assert_eq!(mid, MatchTier::NameSubstring);
    assert!(mid < app);
}
