//! Files for the tree, the editor, and the palette.
//!
//! Three jobs, all of them about the workspace as a *user* sees it rather than
//! as the filesystem does: list a directory the way the tree shows it (ignored
//! paths known, so they can be dimmed or hidden), read and write text with the
//! caveats a text editor owes the reader (binary files, truncation), and search
//! file names and contents the way the palette does — honouring `.gitignore`, so
//! a search never surfaces `target/` or `node_modules/`.
//!
//! Nothing here caches. A directory listing and a content search are fast enough
//! to redo on demand, and a cache that can go stale is a worse editor than a
//! slow one.

use std::cmp::Ordering;
use std::io::Read;
use std::path::{Path, PathBuf};

use grep::matcher::Matcher;
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::WalkBuilder;

/// How much of a file the editor loads before it stops and says so.
pub const TEXT_LOAD_LIMIT: u64 = 2 * 1024 * 1024;

/// How many bytes are sniffed when deciding whether a file is text.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Directories the tree never shows, ignored or not.
const ALWAYS_HIDDEN: [&str; 1] = [".git"];

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Last path component, as shown in the tree.
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
    /// Path relative to the workspace root, with `/` separators.
    pub relative: String,
    /// Whether the entry is a directory (following symlinks).
    pub is_dir: bool,
    /// Size in bytes; zero for directories.
    pub size: u64,
    /// Whether git ignores it.
    pub ignored: bool,
}

/// One directory's entries, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
    /// Path relative to the workspace root, empty for the root itself.
    pub relative: String,
    /// Files first, then directories, each sorted by name.
    pub entries: Vec<FileEntry>,
}

/// A file's text, or the reason there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileText {
    /// Path relative to the workspace root, with `/` separators.
    pub relative: String,
    /// The text, empty for a binary file.
    pub text: String,
    /// Size on disk in bytes.
    pub bytes: u64,
    /// Whether the text was cut at the load limit.
    pub truncated: bool,
    /// Whether the file looks binary rather than text.
    pub binary: bool,
}

/// How a content search should behave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// Whether the pattern respects case.
    pub case_sensitive: bool,
    /// Whether the pattern is a literal string rather than a regular expression.
    pub literal: bool,
    /// How many matches to collect before stopping.
    pub max_results: usize,
    /// Whether dotfiles and ignored paths are searched.
    pub include_hidden: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            literal: true,
            max_results: 200,
            include_hidden: false,
        }
    }
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    /// Path relative to the workspace root, with `/` separators.
    pub path: String,
    /// One-based line number.
    pub line: u64,
    /// The line's text, without its terminator.
    pub text: String,
    /// Byte ranges of the match inside `text`, for highlighting.
    pub ranges: Vec<(usize, usize)>,
}

/// What a search found, and whether it stopped early.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOutcome {
    /// Matches, in walk order.
    pub matches: Vec<SearchMatch>,
    /// How many files were read.
    pub files_searched: usize,
    /// Whether the result limit stopped the search.
    pub truncated: bool,
    /// A pattern the regex engine refused, or a walk that could not start.
    pub error: Option<String>,
}

/// The path of `path` relative to `root`, with `/` separators.
///
/// @param root workspace root
/// @param path absolute path
/// @returns the relative path, empty when `path` is the root itself
pub fn relative(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rest) if rest.as_os_str().is_empty() => String::new(),
        Ok(rest) => slashes(rest),
        Err(_) => slashes(path),
    }
}

/// Rewrites a path's separators for display.
fn slashes(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    if std::path::MAIN_SEPARATOR == '/' {
        text
    } else {
        text.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

/// Lists one directory the way the tree shows it.
///
/// @param root workspace root, the base for ignore files and relative paths
/// @param dir the directory to list
/// @param show_ignored whether git-ignored entries are included (dimmed by the UI)
/// @returns the listing, in display order
/// @throws `Err` when the directory cannot be read
pub fn list_dir(root: &Path, dir: &Path, show_ignored: bool) -> Result<DirListing, String> {
    let matcher = ignore_matcher(root, dir);
    let read = std::fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let mut entries = Vec::new();
    for item in read {
        let Ok(item) = item else { continue };
        let path = item.path();
        let name = item.file_name().to_string_lossy().to_string();
        if ALWAYS_HIDDEN.contains(&name.as_str()) {
            continue;
        }
        let is_dir = path.is_dir();
        let ignored = matcher
            .matched_path_or_any_parents(&path, is_dir)
            .is_ignore();
        if ignored && !show_ignored {
            continue;
        }
        let size = if is_dir {
            0
        } else {
            std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0)
        };
        entries.push(FileEntry {
            name,
            relative: relative(root, &path),
            path,
            is_dir,
            size,
            ignored,
        });
    }
    entries.sort_by(|a, b| {
        // Directories first, then names, case-insensitively, with the raw name
        // breaking ties so the order never depends on the host's collation.
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(DirListing {
        relative: relative(root, dir),
        entries,
    })
}

/// Builds the ignore matcher for a directory: every `.gitignore` and `.ignore`
/// from the workspace root down to it, plus `.git/info/exclude`.
///
/// Each pattern keeps the directory its file lives in as its base, so a nested
/// `.gitignore` applies exactly where git would apply it.
fn ignore_matcher(root: &Path, dir: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    let mut chain: Vec<&Path> = dir
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(root))
        .collect();
    chain.reverse();
    for base in chain {
        for name in [".gitignore", ".ignore"] {
            let candidate = base.join(name);
            if candidate.is_file() {
                builder.add(candidate);
            }
        }
    }
    let exclude = root.join(".git").join("info").join("exclude");
    if exclude.is_file() {
        builder.add(exclude);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

/// Reads a file as text.
///
/// @param root workspace root, for the relative path
/// @param path file to read
/// @param limit how many bytes to load
/// @returns the text, or why there is none
/// @throws `Err` when the path is a directory or cannot be read
pub fn read_text(root: &Path, path: &Path, limit: u64) -> Result<FileText, String> {
    let metadata = std::fs::metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if metadata.is_dir() {
        return Err(format!("{}: is a directory", path.display()));
    }
    let bytes = metadata.len();
    let mut buffer = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(limit).read_to_end(&mut buffer))
        .map_err(|err| format!("{}: {err}", path.display()))?;
    let truncated = bytes > buffer.len() as u64;
    let sniff = &buffer[..buffer.len().min(BINARY_SNIFF_BYTES)];
    let binary = is_binary(sniff);
    Ok(FileText {
        relative: relative(root, path),
        text: if binary {
            String::new()
        } else {
            String::from_utf8_lossy(&buffer).to_string()
        },
        bytes,
        truncated,
        binary,
    })
}

/// Writes a file, creating its parent directories.
///
/// @param path file to write
/// @param text the complete new contents
/// @throws `Err` when the file cannot be written
pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|err| format!("{}: {err}", path.display()))
}

/// Whether a byte sample looks like binary rather than text.
///
/// A NUL byte is the signal git uses, and it is the cheap one: no text file has
/// one, and a UTF-16 or compiled file always does.
///
/// @param sample the first bytes of a file
/// @returns true when the file should not be shown as text
pub fn is_binary(sample: &[u8]) -> bool {
    sample.contains(&0)
}

/// Searches file contents under the workspace root.
///
/// @param root workspace root
/// @param query literal string or regular expression
/// @param options how to match, and how much to collect
/// @returns matches and any pattern error
pub fn search(root: &Path, query: &str, options: &SearchOptions) -> SearchOutcome {
    let mut outcome = SearchOutcome::default();
    if query.is_empty() {
        return outcome;
    }
    let matcher = match RegexMatcherBuilder::new()
        .case_insensitive(!options.case_sensitive)
        .fixed_strings(options.literal)
        .build(query)
    {
        Ok(matcher) => matcher,
        Err(err) => {
            outcome.error = Some(format!("pattern: {err}"));
            return outcome;
        }
    };
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(grep::searcher::BinaryDetection::quit(0))
        .build();
    let mut walk = WalkBuilder::new(root);
    walk.hidden(!options.include_hidden)
        .git_ignore(!options.include_hidden)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .parents(true)
        // A `.gitignore` counts even when the directory is not a repository:
        // the tree honours it either way, and search must agree with the tree.
        .require_git(false);
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let remaining = options.max_results.saturating_sub(outcome.matches.len());
        if remaining == 0 {
            outcome.truncated = true;
            break;
        }
        let mut sink = CollectSink {
            matcher: &matcher,
            path: relative(root, entry.path()),
            limit: remaining,
            matches: Vec::new(),
        };
        // A file that cannot be read is skipped, not reported: a search over a
        // tree with a socket or an unreadable file in it still answers.
        if searcher
            .search_path(&matcher, entry.path(), &mut sink)
            .is_ok()
        {
            outcome.files_searched += 1;
            outcome.matches.extend(sink.matches);
        }
    }
    outcome.truncated |= outcome.matches.len() >= options.max_results;
    outcome
}

/// A sink that keeps the matching lines of one file.
struct CollectSink<'m> {
    matcher: &'m RegexMatcher,
    path: String,
    limit: usize,
    matches: Vec<SearchMatch>,
}

impl Sink for CollectSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        matched: &SinkMatch<'_>,
    ) -> Result<bool, std::io::Error> {
        let bytes = matched.bytes();
        let text = String::from_utf8_lossy(trim_terminator(bytes)).to_string();
        let mut ranges = Vec::new();
        let ceiling = text.len();
        // Ranges are only the highlight; if the matcher cannot run again the
        // line is still a match, so the failure is dropped rather than
        // reported as a search error.
        let _ = self.matcher.find_iter(bytes, |found| {
            let start = found.start().min(ceiling);
            let end = found.end().min(ceiling);
            if start < end {
                ranges.push((start, end));
            }
            true
        });
        self.matches.push(SearchMatch {
            path: self.path.clone(),
            line: matched.line_number().unwrap_or(0),
            text,
            ranges,
        });
        Ok(self.matches.len() < self.limit)
    }
}

/// Drops a line's terminator, so a match's text is one line of text.
fn trim_terminator(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\n' || line[end - 1] == b'\r') {
        end -= 1;
    }
    &line[..end]
}

/// Lists the files under the workspace root, for the palette's quick open.
///
/// @param root workspace root
/// @param limit how many paths to collect
/// @param include_hidden whether ignored and dot paths are listed
/// @returns relative paths, sorted
pub fn list_files(root: &Path, limit: usize, include_hidden: bool) -> Vec<String> {
    let mut walk = WalkBuilder::new(root);
    walk.hidden(!include_hidden)
        .git_ignore(!include_hidden)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .parents(true)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b));
    let mut files = Vec::new();
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if files.len() >= limit {
            break;
        }
        files.push(relative(root, entry.path()));
    }
    files.sort();
    files
}

/// Scores a fuzzy match of `pattern` against `candidate`, the way a palette
/// ranks paths.
///
/// A contiguous substring beats a scattered subsequence, a match at a path or
/// word boundary beats one in the middle of a name, and a shorter candidate
/// beats a longer one when everything else is equal.
///
/// @param pattern what the user typed
/// @param candidate the path being ranked
/// @returns a score, higher first, or `None` when the pattern does not match
pub fn fuzzy_score(pattern: &str, candidate: &str) -> Option<i32> {
    if pattern.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = candidate.chars().collect();
    let lowered: Vec<char> = hay.iter().map(|c| c.to_ascii_lowercase()).collect();
    let needle: Vec<char> = pattern.chars().map(|c| c.to_ascii_lowercase()).collect();

    // A contiguous run anywhere in the candidate is the strongest signal, and
    // the earlier it starts, the better.
    let mut score = 0;
    let contiguous = lowered
        .windows(needle.len().min(lowered.len().max(1)))
        .position(|window| window == needle.as_slice());
    match contiguous {
        Some(at) => score += 100 - (at as i32).min(90) / 2,
        None => {
            let mut cursor = 0usize;
            let mut last: Option<usize> = None;
            for (index, want) in needle.iter().enumerate() {
                let found = (cursor..hay.len()).find(|at| lowered[*at] == *want)?;
                if index > 0 && last == Some(found.wrapping_sub(1)) {
                    score += 8;
                }
                if found == 0 || is_boundary(hay[found - 1]) {
                    score += 12;
                }
                score += 1;
                last = Some(found);
                cursor = found + 1;
            }
        }
    }
    // Prefer the shorter of two equally good candidates: the same pattern in
    // `src/term.rs` is a better answer than in `src/terminal/emulator/mod.rs`.
    Some(score - (hay.len() as i32) / 8)
}

/// Whether a character ends a word, for fuzzy scoring.
fn is_boundary(previous: char) -> bool {
    matches!(previous, '/' | '\\' | '.' | '_' | '-' | ' ') || previous.is_ascii_uppercase()
}

/// Compares two relative paths the way the tree presents them.
///
/// @param a left path
/// @param b right path
/// @returns the ordering
pub fn compare_paths(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace-shaped directory that removes itself.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("harness-fs-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&root).ok();
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, path: &str, contents: &str) -> PathBuf {
            let full = self.root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
            full
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    #[test]
    fn a_listing_is_directories_first_then_names_and_marks_ignored_entries() {
        let fixture = Fixture::new("listing");
        fixture.write(".gitignore", "build/\n*.log\n");
        fixture.write("src/main.rs", "fn main() {}\n");
        fixture.write("build/object.o", "binary\n");
        fixture.write("session.log", "noise\n");
        fixture.write("README.md", "# read me\n");

        let listing = list_dir(&fixture.root, &fixture.root, false).expect("listing");
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["src", ".gitignore", "README.md"],
            "ignored paths are left out"
        );
        assert!(listing.entries[0].is_dir);
        assert_eq!(listing.entries[0].relative, "src");

        let listing = list_dir(&fixture.root, &fixture.root, true).expect("listing with ignored");
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["build", "src", ".gitignore", "README.md", "session.log"]
        );
        let build = listing
            .entries
            .iter()
            .find(|entry| entry.name == "build")
            .unwrap();
        assert!(build.ignored, "an ignored directory is marked, not hidden");
        let readme = listing
            .entries
            .iter()
            .find(|entry| entry.name == "README.md")
            .unwrap();
        assert!(!readme.ignored);
        assert_eq!(readme.size, 10);
    }

    #[test]
    fn a_nested_ignore_file_applies_where_git_would_apply_it() {
        let fixture = Fixture::new("nested-ignore");
        fixture.write(".gitignore", "*.log\n");
        fixture.write("app/.gitignore", "generated/\n");
        fixture.write("app/generated/schema.rs", "// generated\n");
        fixture.write("app/src/lib.rs", "pub fn lib() {}\n");
        fixture.write("app/notes.log", "noise\n");

        let listing = list_dir(&fixture.root, &fixture.root.join("app"), false).expect("listing");
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        // `app/notes.log` is ignored by the root's file, `app/generated/` by the
        // one next to it; `src` and the ignore file itself survive both.
        assert_eq!(names, vec!["src", ".gitignore"]);
        assert_eq!(listing.relative, "app");

        let inner =
            list_dir(&fixture.root, &fixture.root.join("app/src"), false).expect("listed deeper");
        let inner_names: Vec<&str> = inner
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(inner_names, vec!["lib.rs"]);
    }

    #[test]
    fn reading_reports_truncation_and_refuses_to_call_binary_text() {
        let fixture = Fixture::new("reading");
        let long = fixture.write("long.txt", &"x".repeat(64));
        let text = read_text(&fixture.root, &long, 16).expect("read");
        assert!(text.truncated);
        assert_eq!(text.text.len(), 16);
        assert_eq!(text.bytes, 64);
        assert!(!text.binary);
        assert_eq!(text.relative, "long.txt");

        let binary = fixture.write("data.bin", "\u{0}\u{1}\u{2}");
        let text = read_text(&fixture.root, &binary, TEXT_LOAD_LIMIT).expect("read");
        assert!(
            text.binary,
            "a NUL byte means the editor shows a notice, not the bytes"
        );
        assert!(text.text.is_empty());

        let error = read_text(&fixture.root, &fixture.root, TEXT_LOAD_LIMIT)
            .expect_err("a directory is not readable");
        assert!(error.contains("is a directory"));
    }

    #[test]
    fn writing_creates_the_directories_a_new_file_needs() {
        let fixture = Fixture::new("writing");
        let path = fixture.root.join("deep/nested/new.txt");
        write_text(&path, "hello\n").expect("write");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[test]
    fn a_content_search_finds_lines_ranges_and_skips_ignored_paths() {
        let fixture = Fixture::new("search");
        fixture.write(".gitignore", "target/\n");
        fixture.write("src/lib.rs", "fn alpha() {}\nlet beta = alpha();\n");
        fixture.write("target/generated.rs", "alpha alpha alpha\n");

        let options = SearchOptions {
            max_results: 50,
            ..SearchOptions::default()
        };
        let outcome = search(&fixture.root, "alpha", &options);
        assert!(outcome.error.is_none(), "got {:?}", outcome.error);
        assert_eq!(outcome.matches.len(), 2, "the ignored file is not searched");
        assert!(outcome
            .matches
            .iter()
            .all(|found| found.path == "src/lib.rs"));
        assert_eq!(outcome.matches[0].line, 1);
        assert_eq!(outcome.matches[0].text, "fn alpha() {}");
        assert_eq!(outcome.matches[0].ranges, vec![(3, 8)]);
        assert_eq!(outcome.matches[1].line, 2);
        assert_eq!(outcome.matches[1].ranges, vec![(11, 16)]);
        assert!(outcome.files_searched >= 1);

        let searched = search(
            &fixture.root,
            "alpha",
            &SearchOptions {
                include_hidden: true,
                ..options.clone()
            },
        );
        assert_eq!(
            searched.matches.len(),
            3,
            "the ignored file is searched when asked"
        );
    }

    #[test]
    fn a_search_respects_case_and_literal_mode_and_reports_a_bad_pattern() {
        let fixture = Fixture::new("search-options");
        fixture.write("src/lib.rs", "Alpha\nalpha\n");
        let case_sensitive = SearchOptions {
            case_sensitive: true,
            ..SearchOptions::default()
        };
        assert_eq!(
            search(&fixture.root, "Alpha", &case_sensitive)
                .matches
                .len(),
            1
        );
        assert_eq!(
            search(&fixture.root, "alpha", &case_sensitive)
                .matches
                .len(),
            1
        );

        let regex = SearchOptions {
            literal: false,
            max_results: 50,
            ..SearchOptions::default()
        };
        assert_eq!(
            search(&fixture.root, "a.ph.", &regex).matches.len(),
            2,
            "a regex matches both lines"
        );

        let broken = search(&fixture.root, "a(", &regex);
        assert!(
            broken.error.is_some(),
            "an unclosed group is reported, not panicked on"
        );
        assert!(broken.matches.is_empty());
    }

    #[test]
    fn a_long_result_set_is_cut_at_the_limit_and_says_so() {
        let fixture = Fixture::new("limit");
        fixture.write("many.txt", &"needle\n".repeat(20));
        let options = SearchOptions {
            max_results: 5,
            ..SearchOptions::default()
        };
        let outcome = search(&fixture.root, "needle", &options);
        assert_eq!(outcome.matches.len(), 5);
        assert!(outcome.truncated);
    }

    #[test]
    fn listing_files_walks_the_workspace_once_and_sorts() {
        let fixture = Fixture::new("palette");
        fixture.write(".gitignore", "vendor/\n");
        fixture.write("src/main.rs", "");
        fixture.write("src/term.rs", "");
        fixture.write("vendor/dep.rs", "");
        let files = list_files(&fixture.root, 100, false);
        assert_eq!(
            files,
            vec!["src/main.rs", "src/term.rs"],
            "dotfiles stay out of the palette by default"
        );
        assert_eq!(
            list_files(&fixture.root, 100, true),
            vec![".gitignore", "src/main.rs", "src/term.rs", "vendor/dep.rs"]
        );
    }

    #[test]
    fn fuzzy_scores_a_contiguous_match_above_a_scattered_one() {
        let contiguous = fuzzy_score("term", "src/term.rs").unwrap();
        let scattered = fuzzy_score("term", "src/t_erm_handler.rs").unwrap();
        assert!(
            contiguous > scattered,
            "contiguous {contiguous} vs scattered {scattered}"
        );
        assert!(fuzzy_score("term", "crates/harness-core/src/term.rs").unwrap() > 0);
        assert_eq!(fuzzy_score("zzz", "src/term.rs"), None);
        assert_eq!(
            fuzzy_score("term", "src/tree/mod.rs"),
            None,
            "the letters are there, the order is not"
        );
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert!(
            fuzzy_score("TERM", "src/term.rs").is_some(),
            "the score ignores case"
        );
        // Shorter candidates win when the match is equally good.
        assert!(
            fuzzy_score("main", "src/main.rs").unwrap()
                > fuzzy_score("main", "src/deeply/nested/main.rs").unwrap()
        );
    }

    #[test]
    fn relative_paths_are_slash_separated_and_the_root_is_empty() {
        let fixture = Fixture::new("relative");
        assert_eq!(relative(&fixture.root, &fixture.root), "");
        assert_eq!(
            relative(&fixture.root, &fixture.root.join("src/lib.rs")),
            "src/lib.rs"
        );
        let outside = Path::new("/tmp/elsewhere.rs");
        assert_eq!(relative(&fixture.root, outside), "/tmp/elsewhere.rs");
    }
}
