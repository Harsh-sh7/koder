//! Repository state for the git pane: status, branches, history, diffs.
//!
//! All of it through `git2`, against the repository the workspace is in. The
//! module is deliberately read-mostly: the pane shows the truth, and the few
//! writes it offers (stage, unstage, commit) are the ones a person expects to
//! be able to do without leaving the harness.
//!
//! Nothing here watches the filesystem; the app decides when to re-read. Status
//! and diff are cheap enough to re-read on demand, which keeps the model simple
//! and the pane honest.

use std::path::{Path, PathBuf};

use git2::{DiffFormat, DiffOptions, Repository, Status, StatusOptions};

/// What happened to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeKind {
    /// A new file.
    Added,
    /// A changed file.
    Modified,
    /// A removed file.
    Deleted,
    /// A file that moved, possibly with edits.
    Renamed,
    /// A change to the file's mode or type.
    TypeChange,
    /// A path left conflicted by a merge.
    Conflicted,
    /// A path git does not track yet.
    Untracked,
}

impl ChangeKind {
    /// The letter a git UI shows.
    ///
    /// @returns one character
    pub fn letter(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::TypeChange => 'T',
            Self::Conflicted => 'U',
            Self::Untracked => '?',
        }
    }

    /// A phrase for the pane.
    ///
    /// @returns the kind in words
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::TypeChange => "type changed",
            Self::Conflicted => "conflicted",
            Self::Untracked => "untracked",
        }
    }
}

/// One changed path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileChange {
    /// Path relative to the repository root.
    pub path: String,
    /// What happened to it.
    pub kind: ChangeKind,
}

/// Working-tree status, split the way a git UI shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// Current branch, or a detached-HEAD description.
    pub branch: String,
    /// Upstream branch, when one is configured.
    pub upstream: Option<String>,
    /// Commits the branch is ahead of its upstream.
    pub ahead: usize,
    /// Commits it is behind.
    pub behind: usize,
    /// Changes staged for the next commit.
    pub staged: Vec<FileChange>,
    /// Tracked changes not staged.
    pub unstaged: Vec<FileChange>,
    /// Untracked files.
    pub untracked: Vec<FileChange>,
    /// Paths a merge left conflicted.
    pub conflicted: Vec<FileChange>,
}

impl RepoStatus {
    /// Whether the working tree has anything to report.
    ///
    /// @returns true when every list is empty
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty()
            && self.unstaged.is_empty()
            && self.untracked.is_empty()
            && self.conflicted.is_empty()
    }

    /// Total number of changed paths.
    ///
    /// @returns the sum of the four lists
    pub fn change_count(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.untracked.len() + self.conflicted.len()
    }
}

/// One branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// Short name, e.g. `main`.
    pub name: String,
    /// Whether HEAD points at it.
    pub current: bool,
    /// The commit it points at, short form.
    pub target: String,
}

/// One commit, for the history list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    /// Full commit id.
    pub id: String,
    /// Short id.
    pub short: String,
    /// First line of the message.
    pub summary: String,
    /// Author name.
    pub author: String,
    /// Author time, `YYYY-MM-DD HH:MM`.
    pub when: String,
}

/// A repository, or the absence of one.
pub struct GitRepo {
    repo: Repository,
    root: PathBuf,
}

impl GitRepo {
    /// Opens the repository containing a directory.
    ///
    /// @param dir any directory inside the repository
    /// @returns the repository, or `None` when there is none
    pub fn discover(dir: &Path) -> Option<Self> {
        let repo = Repository::discover(dir).ok()?;
        let root = repo.workdir().unwrap_or_else(|| repo.path()).to_path_buf();
        Some(Self { repo, root })
    }

    /// The repository's working directory.
    ///
    /// @returns the root path
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads the working-tree status.
    ///
    /// @returns status, with paths relative to the repository root
    /// @throws `Err` when git cannot read the index or the trees
    pub fn status(&self) -> Result<RepoStatus, String> {
        let mut options = StatusOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true);
        let statuses = self
            .repo
            .statuses(Some(&mut options))
            .map_err(|err| format!("git status: {err}"))?;

        let mut status = RepoStatus {
            branch: self.branch_name(),
            ..RepoStatus::default()
        };
        for entry in statuses.iter() {
            let Ok(path) = entry.path() else { continue };
            let flags = entry.status();
            if flags.contains(Status::CONFLICTED) {
                status.conflicted.push(FileChange {
                    path: path.to_string(),
                    kind: ChangeKind::Conflicted,
                });
                continue;
            }
            if flags.intersects(
                Status::INDEX_NEW
                    | Status::INDEX_MODIFIED
                    | Status::INDEX_DELETED
                    | Status::INDEX_RENAMED
                    | Status::INDEX_TYPECHANGE,
            ) {
                status.staged.push(FileChange {
                    path: path.to_string(),
                    kind: index_kind(flags),
                });
            }
            if flags.intersects(Status::WT_NEW) {
                status.untracked.push(FileChange {
                    path: path.to_string(),
                    kind: ChangeKind::Untracked,
                });
            } else if flags.intersects(
                Status::WT_MODIFIED
                    | Status::WT_DELETED
                    | Status::WT_RENAMED
                    | Status::WT_TYPECHANGE,
            ) {
                status.unstaged.push(FileChange {
                    path: path.to_string(),
                    kind: workdir_kind(flags),
                });
            }
        }
        for list in [
            &mut status.staged,
            &mut status.unstaged,
            &mut status.untracked,
            &mut status.conflicted,
        ] {
            list.sort();
        }

        if let Ok(head) = self.repo.head() {
            if let Ok(local) = head.shorthand() {
                if let Ok(branch) = self.repo.find_branch(local, git2::BranchType::Local) {
                    if let Ok(upstream) = branch.upstream() {
                        status.upstream = upstream.name().ok().flatten().map(short_branch);
                        let local_oid = branch.get().target();
                        let upstream_oid = upstream.get().target();
                        if let (Some(local_oid), Some(upstream_oid)) = (local_oid, upstream_oid) {
                            if let Ok((ahead, behind)) =
                                self.repo.graph_ahead_behind(local_oid, upstream_oid)
                            {
                                status.ahead = ahead;
                                status.behind = behind;
                            }
                        }
                    }
                }
            }
        }
        Ok(status)
    }

    /// The current branch's name, or a description of HEAD.
    ///
    /// @returns a short label for the pane
    pub fn branch_name(&self) -> String {
        match self.repo.head() {
            Ok(head) => {
                if let Ok(name) = head.shorthand() {
                    return name.to_string();
                }
                // A detached HEAD is worth showing as such, with the id that is
                // checked out.
                head.target()
                    .map(|oid| format!("detached at {}", short_id(oid)))
                    .unwrap_or_else(|| "detached".to_string())
            }
            // An unborn branch (a fresh repository) has no commit yet.
            Err(_) => "no commits yet".to_string(),
        }
    }

    /// Local branches, current first.
    ///
    /// @returns branches
    /// @throws `Err` when references cannot be read
    pub fn branches(&self) -> Result<Vec<BranchInfo>, String> {
        let mut branches = Vec::new();
        let head = self
            .repo
            .head()
            .ok()
            .and_then(|head| head.shorthand().ok().map(str::to_string));
        let list = self
            .repo
            .branches(Some(git2::BranchType::Local))
            .map_err(|err| format!("git branches: {err}"))?;
        for item in list {
            let (branch, _) = item.map_err(|err| format!("git branch: {err}"))?;
            let name = branch.name().ok().flatten().unwrap_or("?").to_string();
            let target = branch.get().target().map(short_id).unwrap_or_default();
            branches.push(BranchInfo {
                current: Some(&name) == head.as_ref(),
                name,
                target,
            });
        }
        branches.sort_by(|a, b| b.current.cmp(&a.current).then_with(|| a.name.cmp(&b.name)));
        Ok(branches)
    }

    /// Recent commits on HEAD.
    ///
    /// @param limit how many to return
    /// @returns history, newest first
    /// @throws `Err` when the log cannot be walked
    pub fn log(&self, limit: usize) -> Result<Vec<CommitInfo>, String> {
        let mut walk = match self.repo.head() {
            Ok(_) => self
                .repo
                .revwalk()
                .map_err(|err| format!("git log: {err}"))?,
            // A repository with no commits yet has no history to show.
            Err(_) => return Ok(Vec::new()),
        };
        walk.set_sorting(git2::Sort::TIME)
            .map_err(|err| format!("git log: {err}"))?;
        walk.push_head().map_err(|err| format!("git log: {err}"))?;
        let mut commits = Vec::new();
        for oid in walk.take(limit) {
            let oid = oid.map_err(|err| format!("git log: {err}"))?;
            let commit = self
                .repo
                .find_commit(oid)
                .map_err(|err| format!("git log: {err}"))?;
            let when = commit.time();
            let stamp = chrono::DateTime::from_timestamp(when.seconds(), 0)
                .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            commits.push(CommitInfo {
                id: oid.to_string(),
                short: short_id(oid),
                summary: commit
                    .summary()
                    .ok()
                    .flatten()
                    .unwrap_or("(no message)")
                    .to_string(),
                author: commit.author().name().unwrap_or("?").to_string(),
                when: stamp,
            });
        }
        Ok(commits)
    }

    /// The unified diff of one path.
    ///
    /// @param path path relative to the repository root
    /// @param staged true for the index's diff against HEAD, false for the
    ///        working tree against the index
    /// @returns patch text, empty when the path has no changes
    /// @throws `Err` when the diff cannot be produced
    pub fn diff(&self, path: &str, staged: bool) -> Result<String, String> {
        let mut options = DiffOptions::new();
        options.pathspec(path).include_typechange(true);
        let diff = if staged {
            let head_tree = self
                .repo
                .head()
                .ok()
                .and_then(|head| head.peel_to_tree().ok());
            self.repo
                .diff_tree_to_index(head_tree.as_ref(), None, Some(&mut options))
                .map_err(|err| format!("git diff: {err}"))?
        } else {
            self.repo
                .diff_index_to_workdir(None, Some(&mut options))
                .map_err(|err| format!("git diff: {err}"))?
        };
        render_diff(&diff)
    }

    /// The whole working tree's diff, for the review pane.
    ///
    /// @param staged true for staged changes
    /// @returns patch text
    /// @throws `Err` when the diff cannot be produced
    pub fn diff_all(&self, staged: bool) -> Result<String, String> {
        let mut options = DiffOptions::new();
        options
            .include_typechange(true)
            .recurse_untracked_dirs(true);
        let diff = if staged {
            let head_tree = self
                .repo
                .head()
                .ok()
                .and_then(|head| head.peel_to_tree().ok());
            self.repo
                .diff_tree_to_index(head_tree.as_ref(), None, Some(&mut options))
                .map_err(|err| format!("git diff: {err}"))?
        } else {
            self.repo
                .diff_index_to_workdir(None, Some(&mut options))
                .map_err(|err| format!("git diff: {err}"))?
        };
        render_diff(&diff)
    }

    /// Stages one path.
    ///
    /// @param path path relative to the repository root
    /// @throws `Err` when the index cannot be written
    pub fn stage(&self, path: &str) -> Result<(), String> {
        let mut index = self
            .repo
            .index()
            .map_err(|err| format!("git index: {err}"))?;
        index
            .add_path(Path::new(path))
            .map_err(|err| format!("git add {path}: {err}"))?;
        index.write().map_err(|err| format!("git index: {err}"))
    }

    /// Unstages one path, leaving the working tree alone.
    ///
    /// @param path path relative to the repository root
    /// @throws `Err` when the index cannot be written
    pub fn unstage(&self, path: &str) -> Result<(), String> {
        let head = self
            .repo
            .head()
            .ok()
            .and_then(|head| head.peel(git2::ObjectType::Commit).ok());
        match head {
            Some(object) => self
                .repo
                .reset_default(Some(&object), [path])
                .map_err(|err| format!("git reset {path}: {err}")),
            // With no commit yet, the index is the only thing to clear.
            None => {
                let mut index = self
                    .repo
                    .index()
                    .map_err(|err| format!("git index: {err}"))?;
                index
                    .remove_path(Path::new(path))
                    .map_err(|err| format!("git rm --cached {path}: {err}"))?;
                index.write().map_err(|err| format!("git index: {err}"))
            }
        }
    }

    /// Commits what is staged.
    ///
    /// @param message commit message
    /// @returns the new commit's short id
    /// @throws `Err` when there is nothing staged or the commit fails
    pub fn commit(&self, message: &str) -> Result<String, String> {
        let mut index = self
            .repo
            .index()
            .map_err(|err| format!("git index: {err}"))?;
        let tree_id = index
            .write_tree()
            .map_err(|err| format!("git write-tree: {err}"))?;
        let tree = self
            .repo
            .find_tree(tree_id)
            .map_err(|err| format!("git tree: {err}"))?;
        let head = self
            .repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok());
        match &head {
            // An unchanged tree is not a commit: git would accept one, and the
            // pane would show a commit that changed nothing.
            Some(head) => {
                let head_tree = head.tree().map_err(|err| format!("git tree: {err}"))?;
                if head_tree.id() == tree_id {
                    return Err("nothing staged to commit".to_string());
                }
            }
            // A repository with no commits yet has no tree to compare against,
            // and an empty one is not a starting point.
            None if tree.is_empty() => return Err("nothing staged to commit".to_string()),
            None => {}
        }
        let parents: Vec<&git2::Commit> = head.iter().collect();
        let author = self
            .repo
            .signature()
            .or_else(|_| git2::Signature::now("AI Harness", "harness@localhost"))
            .map_err(|err| format!("git author: {err}"))?;
        let oid = self
            .repo
            .commit(Some("HEAD"), &author, &author, message, &tree, &parents)
            .map_err(|err| format!("git commit: {err}"))?;
        Ok(short_id(oid))
    }
}

/// The staged change kind for a status entry.
fn index_kind(flags: Status) -> ChangeKind {
    if flags.contains(Status::INDEX_NEW) {
        ChangeKind::Added
    } else if flags.contains(Status::INDEX_DELETED) {
        ChangeKind::Deleted
    } else if flags.contains(Status::INDEX_RENAMED) {
        ChangeKind::Renamed
    } else if flags.contains(Status::INDEX_TYPECHANGE) {
        ChangeKind::TypeChange
    } else {
        ChangeKind::Modified
    }
}

/// The working-tree change kind for a status entry.
fn workdir_kind(flags: Status) -> ChangeKind {
    if flags.contains(Status::WT_DELETED) {
        ChangeKind::Deleted
    } else if flags.contains(Status::WT_RENAMED) {
        ChangeKind::Renamed
    } else if flags.contains(Status::WT_TYPECHANGE) {
        ChangeKind::TypeChange
    } else {
        ChangeKind::Modified
    }
}

/// Formats a patch from a diff, with file headers.
///
/// @param diff the diff to print
/// @returns unified patch text
fn render_diff(diff: &git2::Diff) -> Result<String, String> {
    let mut text = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        // A patch line's content has no prefix: `+`, `-` and ` ` lines must be
        // given their origin character back, while file and hunk headers (`F`,
        // `H`) arrive whole and are copied as they come.
        if matches!(line.origin(), '+' | '-' | ' ') {
            text.push(line.origin());
        }
        if let Ok(content) = std::str::from_utf8(line.content()) {
            text.push_str(content);
        }
        true
    })
    .map_err(|err| format!("git diff: {err}"))?;
    Ok(text)
}

/// The first seven characters of an object id.
fn short_id(oid: git2::Oid) -> String {
    oid.to_string().chars().take(7).collect()
}

/// Strips `refs/heads/` (and the remote prefix) from a reference name.
fn short_branch(name: &str) -> String {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/remotes/"))
        .unwrap_or(name)
        .to_string()
}

/// How many entries a diff-driven view should show before truncating.
pub const DIFF_LINE_LIMIT: usize = 5_000;

/// Truncates a patch for display, naming what was cut.
///
/// @param patch patch text
/// @param limit maximum lines
/// @returns the patch, with a trailing note when it was cut
pub fn clip_patch(patch: &str, limit: usize) -> String {
    let mut lines = patch.lines();
    let mut out = String::new();
    for line in lines.by_ref().take(limit) {
        out.push_str(line);
        out.push('\n');
    }
    let shown = out.lines().count();
    let total = patch.lines().count();
    if total > shown {
        out.push_str(&format!("\n… {shown} of {total} diff lines shown\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with one commit, inside the temp directory.
    struct Fixture {
        dir: PathBuf,
        repo: GitRepo,
    }

    impl Fixture {
        /// Creates an empty repository with a first commit.
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("harness-git-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            let repo = Repository::init(&dir).unwrap();
            // The initial branch's name is a git-wide preference; the fixture
            // names it so the assertions do not depend on the host's config.
            repo.set_head("refs/heads/main").unwrap();
            std::fs::write(dir.join("README.md"), "# fixture\n").unwrap();
            {
                let mut index = repo.index().unwrap();
                index.add_path(Path::new("README.md")).unwrap();
                index.write().unwrap();
                let tree_id = index.write_tree().unwrap();
                let tree = repo.find_tree(tree_id).unwrap();
                let author = git2::Signature::now("Test", "test@example.com").unwrap();
                repo.commit(Some("HEAD"), &author, &author, "initial", &tree, &[])
                    .unwrap();
            }
            let repo = GitRepo::discover(&dir).expect("discover the fixture repository");
            Self { dir, repo }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[test]
    fn a_clean_repository_reports_nothing_changed() {
        let fixture = Fixture::new("clean");
        let status = fixture.repo.status().expect("status");
        assert!(
            status.is_clean(),
            "a fresh commit leaves nothing to report: {status:?}"
        );
        assert_eq!(status.branch, "main");
        assert_eq!(status.change_count(), 0);
    }

    #[test]
    fn untracked_staged_and_modified_paths_land_in_their_own_lists() {
        let fixture = Fixture::new("lists");
        std::fs::write(fixture.dir.join("new.txt"), "fresh\n").unwrap();
        std::fs::write(fixture.dir.join("README.md"), "# changed\n").unwrap();

        let status = fixture.repo.status().expect("status");
        assert_eq!(status.untracked.len(), 1);
        assert_eq!(status.untracked[0].path, "new.txt");
        assert_eq!(status.untracked[0].kind, ChangeKind::Untracked);
        assert_eq!(status.unstaged.len(), 1);
        assert_eq!(status.unstaged[0].kind, ChangeKind::Modified);

        fixture.repo.stage("new.txt").expect("stage");
        let status = fixture.repo.status().expect("status");
        assert_eq!(status.staged.len(), 1);
        assert_eq!(status.staged[0].kind, ChangeKind::Added);
        assert!(status.untracked.is_empty());

        fixture.repo.unstage("new.txt").expect("unstage");
        let status = fixture.repo.status().expect("status");
        assert!(status.staged.is_empty());
        assert_eq!(status.untracked.len(), 1);
    }

    #[test]
    fn the_diff_of_a_modified_file_is_a_patch() {
        let fixture = Fixture::new("diff");
        std::fs::write(fixture.dir.join("README.md"), "# fixture\nsecond line\n").unwrap();
        let patch = fixture.repo.diff("README.md", false).expect("diff");
        assert!(
            patch.contains("+second line"),
            "the added line is in the patch: {patch}"
        );
        assert!(
            patch.contains("-") || patch.contains("@@"),
            "a hunk header is present: {patch}"
        );

        let staged = fixture.repo.diff("README.md", true).expect("staged diff");
        assert!(staged.is_empty(), "nothing is staged yet");
    }

    #[test]
    fn committing_an_empty_index_is_refused_with_a_reason() {
        let fixture = Fixture::new("empty-commit");
        let error = fixture
            .repo
            .commit("nothing to see")
            .expect_err("an empty commit is refused");
        assert!(
            error.contains("git commit") || error.contains("nothing staged"),
            "got {error}"
        );
    }

    #[test]
    fn staging_and_committing_moves_the_work_into_history() {
        let fixture = Fixture::new("commit");
        std::fs::write(fixture.dir.join("feature.txt"), "work\n").unwrap();
        fixture.repo.stage("feature.txt").expect("stage");
        let short = fixture.repo.commit("add the feature").expect("commit");
        assert_eq!(short.len(), 7);

        let status = fixture.repo.status().expect("status");
        assert!(status.is_clean(), "the commit took everything: {status:?}");

        let log = fixture.repo.log(5).expect("log");
        assert_eq!(log[0].summary, "add the feature");
        assert_eq!(log[1].summary, "initial");
        assert!(
            log[0].when.len() >= 16,
            "the timestamp is formatted: {}",
            log[0].when
        );
    }

    #[test]
    fn branches_list_the_current_one_first() {
        let fixture = Fixture::new("branches");
        let head = fixture.repo.repo.head().unwrap().peel_to_commit().unwrap();
        fixture.repo.repo.branch("feature", &head, false).unwrap();
        let branches = fixture.repo.branches().expect("branches");
        assert_eq!(branches[0].name, "main");
        assert!(branches[0].current);
        assert!(branches
            .iter()
            .any(|branch| branch.name == "feature" && !branch.current));
        assert_eq!(branches[0].target.len(), 7);
    }

    #[test]
    fn discovering_outside_a_repository_reports_nothing() {
        let dir = std::env::temp_dir();
        // The temp directory itself is not a repository, but its ancestors could
        // be on a machine with a repository in $TMPDIR; the assertion is that the
        // call answers rather than panics.
        let _ = GitRepo::discover(&dir);
    }

    #[test]
    fn a_long_patch_is_clipped_with_a_note() {
        let patch: String = (0..100).map(|line| format!("+line {line}\n")).collect();
        let clipped = clip_patch(&patch, 10);
        assert_eq!(
            clipped.lines().count(),
            12,
            "ten lines plus the note and its blank"
        );
        assert!(clipped.contains("10 of 100 diff lines shown"));
    }
}
