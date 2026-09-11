//! The filesystem search builder behind `fs.finder`, shared by both `fs`
//! implementations.
//!
//! This is the one traversal in the stdlib built for *scale*: `fs.list` answers
//! about a single directory and `fs.walk` hands back a whole tree for the script
//! to pick through, but a finder is told what it is looking for up front and so
//! can decline to descend at all. The walk is [`ignore::WalkBuilder`] — the
//! engine under `fd` and `rg` — which is what buys the pruning, the ignore-file
//! semantics and the symlink-cycle handling that a hand-rolled `read_dir`
//! recursion has to reimplement badly.
//!
//! **The default answers the smallest honest set.** Ignore files are respected
//! and hidden entries skipped, so a finder over a source tree never walks
//! `.git/`, and never walks `target/` or `node_modules/` when a `.gitignore`
//! says not to. That is not merely a filter over the results: an ignored
//! directory is never descended into, so the cost of a pruned subtree is one
//! `stat`, not its whole contents. Scripts that need the rest opt back in by
//! *level*, the way `fd` does:
//!
//! | Level | Method | Adds |
//! | --- | --- | --- |
//! | default | — | — |
//! | `fd -H` | [`hidden`](FinderState::hidden) | dotfiles and dot-directories |
//! | `fd -I` | `no_ignore` | entries an ignore file excluded |
//! | `fd -u` | `unrestricted` | both |
//!
//! **Filters say what they match against.** The predecessor auto-detected it
//! from the pattern — a glob containing `/` or `**` matched the path, anything
//! else the basename — which made `:name()` and `:glob()` the same function
//! wearing two names, and quietly turned `:name("a/b")` into a path match. Here
//! [`Match::Name`] and [`Match::Path`] are separate methods that always mean
//! what they say.
//!
//! **Repeating a filter widens it; different filters narrow.** Every
//! [`Clause`] in [`FinderState::clauses`] must match (AND), but a clause holds
//! alternatives that OR together, so `:ext("rs", "toml")` and
//! `:name("*.rs"):name("*.toml")` both mean "either extension" — the disjunction
//! the old all-globs-must-match shape could only express by packing braces into
//! one dense pattern. `:ext("")` is the alternative with no glob spelling —
//! [`Pattern::NoExt`], since a glob can require a dot but not forbid one.
//!
//! Excludes are different in kind from the negated globs they replace: an
//! `:exclude()` matching a directory **prunes** it, so the subtree is never
//! walked rather than walked and discarded.
//!
//! **Content search is a terminal, not a filter.** `:grep()` and
//! `:grep_files()` end a chain rather than narrowing it, because they are the
//! only conditions that cost a file *read* where every neighbouring clause
//! costs a `stat` — and a chainable `:grep(a):grep(b)` would owe an answer about
//! whether contents AND or OR the way [`Clause`]s do. Ending the chain also
//! settles the return shape: a filter has to hand back paths, while a terminal
//! is free to report the line and the capture groups, which is the information
//! that makes it worth more than `fs.read` plus `re.find`. Both are host-only
//! (see [`add_methods_with_follow`]). The engine is ripgrep's own
//! `grep-searcher`, so binary files, line endings and mmap are its problem, and
//! the pattern dialect is `regex`'s — the same one `re` takes.

use globset::{Glob, GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::rc::Rc;

type Result<T> = mlua::Result<T>;

/// What a glob is tested against.
#[derive(Clone, Copy, PartialEq)]
pub enum Match {
    /// The entry's basename, at any depth (`:name`, `:ext`).
    Name,
    /// The whole path as the finder reports it (`:path`, `:exclude`).
    Path,
}

/// One compiled alternative within a [`Clause`].
///
/// Almost always a glob; `NoExt` is the one condition globbing cannot state,
/// since a glob can require a dot but never forbid one (see [`Pattern::no_ext`]).
#[derive(Clone)]
enum Pattern {
    Glob {
        matcher: GlobMatcher,
        target: Match,
    },
    /// `:ext("")` — the basename carries no extension.
    NoExt,
}

impl Pattern {
    /// The alternative `:ext("")` contributes. Judged by `Path::extension`,
    /// which is what `path.extension` reports, so the two agree on every entry:
    /// a leading dot is the name's own (`.gitignore` has no extension), and a
    /// trailing one leaves the extension empty but *present* (`odd.` has one,
    /// `odd` does not).
    fn no_ext() -> Self {
        Pattern::NoExt
    }

    fn new(pattern: &str, target: Match) -> Result<Self> {
        if pattern.is_empty() {
            return Err(mlua::Error::runtime("empty glob pattern"));
        }
        // `literal_separator` keeps `*` from crossing a `/`, so `src/*.rs` means
        // one level and `src/**/*.rs` means any -- the distinction is the whole
        // point of a path-targeted pattern, and without it the two collapse.
        // A name-targeted pattern never sees a separator anyway.
        let matcher = GlobBuilder::new(pattern)
            .literal_separator(target == Match::Path)
            .build()
            .map(|g: Glob| g.compile_matcher())
            .map_err(|e| mlua::Error::runtime(format!("invalid glob pattern {pattern:?}: {e}")))?;
        Ok(Pattern::Glob { matcher, target })
    }

    fn is_match(&self, path: &str, name: &str) -> bool {
        match self {
            Pattern::Glob { matcher, target } => match target {
                Match::Name => matcher.is_match(name),
                Match::Path => matcher.is_match(path),
            },
            Pattern::NoExt => Path::new(name).extension().is_none(),
        }
    }
}

/// A group of alternatives that OR together. Each call to a filter method adds
/// one; the clause passes if any of them match.
#[derive(Clone, Default)]
struct Clause {
    patterns: Vec<Pattern>,
}

impl Clause {
    fn is_match(&self, path: &str, name: &str) -> bool {
        self.patterns.iter().any(|p| p.is_match(path, name))
    }
}

/// What kind of entry to keep.
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
}

impl Kind {
    /// Parse the script-facing spelling used by `:type()`.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "file" | "f" => Ok(Kind::File),
            "dir" | "directory" | "d" => Ok(Kind::Dir),
            "symlink" | "link" | "l" => Ok(Kind::Symlink),
            other => Err(mlua::Error::runtime(format!(
                "unknown type {other:?}: expected \"file\", \"dir\" or \"symlink\""
            ))),
        }
    }
}

/// How `:sort()` orders the results.
#[derive(Clone, Copy, PartialEq)]
pub enum SortKey {
    Path,
    Name,
    Size,
    /// Newest first -- the useful direction for "the latest log", and the one
    /// `ls -t` picks. Every other key sorts ascending.
    Modified,
}

impl SortKey {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "path" => Ok(SortKey::Path),
            "name" => Ok(SortKey::Name),
            "size" => Ok(SortKey::Size),
            "mtime" | "modified" => Ok(SortKey::Modified),
            other => Err(mlua::Error::runtime(format!(
                "unknown sort key {other:?}: expected \"path\", \"name\", \"size\" or \"mtime\""
            ))),
        }
    }
}

/// Which entries an ignore-file layer keeps out. Mirrors `fd`'s levels rather
/// than exposing `ignore`'s individual toggles: a script wants "the normal set",
/// "and the dotfiles", "and the ignored ones", not a per-VCS-file matrix.
#[derive(Clone, Copy)]
pub struct Restrictions {
    /// Skip dotfiles and dot-directories. On by default; `:hidden()` clears it.
    pub skip_hidden: bool,
    /// Respect `.gitignore`/`.ignore`. On by default; `:no_ignore()` clears it.
    pub use_ignore_files: bool,
}

impl Default for Restrictions {
    fn default() -> Self {
        Self {
            skip_hidden: true,
            use_ignore_files: true,
        }
    }
}

/// Inclusive depth bounds from the start directory; direct children are depth 1.
#[derive(Clone, Copy, Default)]
pub struct DepthBound {
    pub min: Option<usize>,
    pub max: Option<usize>,
}

impl DepthBound {
    fn contains(&self, depth: usize) -> bool {
        self.min.is_none_or(|m| depth >= m) && self.max.is_none_or(|m| depth <= m)
    }
}

/// Everything a finder accumulates before it runs.
#[derive(Default)]
pub struct FinderState {
    /// Positive clauses, all of which must match.
    clauses: Vec<Clause>,
    /// Exclusions; matching any one drops the entry, and prunes it if it is a
    /// directory.
    excludes: Clause,
    /// Kinds to keep. Empty means every kind.
    kinds: Vec<Kind>,
    depth: DepthBound,
    pub restrictions: Restrictions,
    sort: Option<SortKey>,
    limit: Option<usize>,
    /// Follow symlinks while walking. Off by default: a link is reported under
    /// its own name and not descended into, so a tree of links cannot inflate
    /// the result set.
    follow: bool,
}

/// How much of the walk a terminal needs, so the walk can stop early.
#[derive(Clone, Copy, PartialEq)]
enum Need {
    /// Every match (`:find`, `:count`).
    All,
    /// One match is enough (`:any`) -- existence is decided by the first hit, so
    /// the rest of the tree need not be read. There is deliberately no terminal
    /// that *returns a path* this way: `:find()` is path-sorted, so anything
    /// claiming to be "the first match" would have to agree with `:find()[1]`,
    /// and knowing which entry that is means walking everything anyway.
    One,
}

/// One entry that survived filtering.
pub struct Hit {
    /// The path as the script asked for it (see each `fs`'s renderer).
    pub path: String,
    /// Real absolute path, for the metadata a sort needs.
    real: PathBuf,
}

/// See [`Source::renderer`].
pub type Renderer = std::sync::Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

/// The walk a host supplies. `rfs` walks the host filesystem; `vfs` walks the
/// root and refuses escapes, so the finder itself needs to know neither.
pub trait Source {
    /// Absolute path the walk starts at.
    fn start(&self) -> PathBuf;
    /// A renderer of absolute paths the way this `fs` spells results, returning
    /// `None` to drop one -- which is how the rooted VFS declines a symlink
    /// leaving the root without the finder knowing what a root is. `Send + Sync`
    /// because the walker's prune callback needs it: the walk applies it to
    /// entries *before* descending, so an outward link is never entered.
    fn renderer(&self) -> Renderer;
    /// Whether links are resolved whatever the script asked for.
    ///
    /// The rooted VFS says yes: `ied` are deliberately **not**
    /// symlink-aware, so a link is the thing it points at rather than an object
    /// in its own right, and there is no `:follow()` to ask with. That is safe
    /// precisely because [`renderer`](Source::renderer) is a containment check on
    /// the *resolved* location -- resolving more can only surface files already
    /// inside the root, and a link leaving it is refused whether or not it was
    /// followed. The host filesystem says no and lets `:follow()` decide.
    fn always_follow(&self) -> bool {
        false
    }
}

/// A chainable search. Cheap to clone: clones share one builder state, so
/// `local f = fs.finder(".")` then `f:files()` narrows `f` itself, matching the
/// `hcl` builders.
pub struct Finder<S> {
    pub source: Rc<S>,
    pub state: Rc<RefCell<FinderState>>,
}

/// Hand-written rather than derived: both fields are `Rc`, so a finder clones
/// whether or not `S` does, and `#[derive(Clone)]` would demand `S: Clone` for
/// no reason.
impl<S> Clone for Finder<S> {
    fn clone(&self) -> Self {
        Self {
            source: Rc::clone(&self.source),
            state: Rc::clone(&self.state),
        }
    }
}

impl<S: Source> Finder<S> {
    pub fn new(source: S) -> Self {
        Self {
            source: Rc::new(source),
            state: Rc::new(RefCell::new(FinderState::default())),
        }
    }

    /// Add alternatives to a positive clause. Each call is its own clause, so
    /// repeated calls AND while the patterns within one call OR.
    pub fn push_clause(&self, patterns: &[String], target: Match) -> Result<()> {
        let mut clause = Clause::default();
        for p in patterns {
            clause.patterns.push(Pattern::new(p, target)?);
        }
        if !clause.patterns.is_empty() {
            self.state.borrow_mut().clauses.push(clause);
        }
        Ok(())
    }

    /// Add the alternatives `:ext()` asks for, as one clause. Extensions are
    /// mapped here rather than at the binding because `""` does not become a
    /// glob at all: `*.` would match a name *ending* in a dot, which is the
    /// opposite of having no extension.
    pub fn push_ext_clause(&self, exts: &[String]) -> Result<()> {
        let mut clause = Clause::default();
        for e in exts {
            let e = e.trim_start_matches('.');
            clause.patterns.push(if e.is_empty() {
                Pattern::no_ext()
            } else {
                Pattern::new(&format!("*.{e}"), Match::Name)?
            });
        }
        if !clause.patterns.is_empty() {
            self.state.borrow_mut().clauses.push(clause);
        }
        Ok(())
    }

    /// Add exclusion alternatives. All excludes share one clause: any match
    /// drops the entry, so they OR by nature.
    pub fn push_exclude(&self, patterns: &[String], target: Match) -> Result<()> {
        let mut state = self.state.borrow_mut();
        for p in patterns {
            state.excludes.patterns.push(Pattern::new(p, target)?);
        }
        Ok(())
    }

    pub fn push_kind(&self, kind: Kind) {
        let mut state = self.state.borrow_mut();
        if !state.kinds.contains(&kind) {
            state.kinds.push(kind);
        }
    }

    pub fn set_depth(&self, depth: DepthBound) {
        self.state.borrow_mut().depth = depth;
    }

    pub fn set_sort(&self, key: SortKey) {
        self.state.borrow_mut().sort = Some(key);
    }

    pub fn set_limit(&self, n: usize) {
        self.state.borrow_mut().limit = Some(n);
    }

    pub fn set_follow(&self, yes: bool) {
        self.state.borrow_mut().follow = yes;
    }

    /// Run the walk and return the matches, sorted and limited.
    pub fn find(&self) -> Result<Vec<Hit>> {
        self.run(Need::All)
    }

    /// How many entries match. A limit caps the count, so
    /// `:limit(10):count()` answers "up to ten".
    pub fn count(&self) -> Result<usize> {
        Ok(self.run(Need::All)?.len())
    }

    pub fn any(&self) -> Result<bool> {
        Ok(!self.run(Need::One)?.is_empty())
    }

    /// Search the matched files for `pattern`, returning every matching line.
    ///
    /// `:limit(n)` bounds **matches** here rather than files -- see [`grep`] --
    /// so the walk itself runs unlimited and the bound is applied during the
    /// search, where it can stop reading.
    pub fn grep(&self, pattern: &str) -> Result<Vec<GrepMatch>> {
        let limit = self.state.borrow().limit;
        let hits = self.run_unlimited()?;
        grep(&hits, pattern, GrepNeed::Lines, limit)
    }

    /// The paths whose contents match `pattern` (`rg -l`): each file is read
    /// only as far as its first match, and `:limit(n)` bounds the files
    /// returned -- for this terminal a match *is* a file, so the two readings of
    /// `limit` coincide.
    pub fn grep_files(&self, pattern: &str) -> Result<Vec<String>> {
        let limit = self.state.borrow().limit;
        let hits = self.run_unlimited()?;
        Ok(grep(&hits, pattern, GrepNeed::FirstPerFile, limit)?
            .into_iter()
            .map(|m| m.path)
            .collect())
    }

    /// The walk with `:limit()` left off, for the grep terminals: they spend the
    /// limit on matches, so truncating the *candidate* list first would silently
    /// stop the search early. Sorting still applies -- it decides which files
    /// are reached first, and so which matches a limit keeps.
    fn run_unlimited(&self) -> Result<Vec<Hit>> {
        let saved = {
            let mut s = self.state.borrow_mut();
            s.limit.take()
        };
        let hits = self.run(Need::All);
        self.state.borrow_mut().limit = saved;
        hits
    }

    fn run(&self, need: Need) -> Result<Vec<Hit>> {
        // The state is read into locals up front: the walk below runs Luau-free
        // Rust, but holding a `RefCell` borrow across it would outlive any
        // future reentrancy and is exactly the borrow panic the module docs warn
        // about.
        let (clauses, excludes, kinds, depth, restrictions, sort, limit, follow) = {
            let s = self.state.borrow();
            (
                s.clauses.clone(),
                s.excludes.clone(),
                s.kinds.clone(),
                s.depth,
                s.restrictions,
                s.sort,
                s.limit,
                s.follow,
            )
        };

        let start = self.source.start();
        let render = self.source.renderer();
        // A missing start raises, as `fs.list` does, rather than looking like
        // an empty result.
        if !start.is_dir() {
            let shown = render(&start).unwrap_or_default();
            return Err(mlua::Error::runtime(format!(
                "finder: {}: no such directory",
                if shown.is_empty() { "." } else { &shown }
            )));
        }
        let mut builder = WalkBuilder::new(&start);
        builder
            .hidden(restrictions.skip_hidden)
            .parents(restrictions.use_ignore_files)
            .ignore(restrictions.use_ignore_files)
            .git_ignore(restrictions.use_ignore_files)
            .git_global(restrictions.use_ignore_files)
            .git_exclude(restrictions.use_ignore_files)
            // Respect a `.gitignore` even outside a repository. Without this a
            // tree that merely has the file -- an extracted tarball, a fresh
            // skeleton -- would be walked in full, so the same script would give
            // two different answers depending on whether `.git` happened to be
            // there.
            .require_git(false)
            .follow_links(follow || self.source.always_follow())
            .max_depth(depth.max);

        // Excludes are applied *to the walk* rather than to the results, so a
        // matched directory is never descended into and its whole subtree costs
        // nothing. Filtering after the fact would drop `target` and still walk
        // `target/debug/...`, which is both slower and wrong: an exclude names a
        // region, not a single entry.
        {
            let excludes = excludes.clone();
            let render = render.clone();
            builder.filter_entry(move |entry| {
                // Depth 0 is the start directory: pruning it would empty the
                // search, and the caller already chose it by name.
                if entry.depth() == 0 {
                    return true;
                }
                // A path the renderer declines (a link leaving the rooted VFS)
                // is pruned here, before the walk descends into it.
                let Some(rendered) = render(entry.path()) else {
                    return false;
                };
                // Matched against the same rendered string `:path()` sees.
                let name = entry.file_name().to_string_lossy();
                excludes.patterns.is_empty() || !excludes.is_match(&rendered, &name)
            });
        }
        // Depth bounds prune rather than filter: `max_depth` above stops the
        // descent, so a `:max_depth(1)` over a deep tree costs one directory
        // read. `min` has no pruning form -- a shallow entry must still be
        // walked through to reach the deep ones -- so it is applied per entry.

        // A single-threaded walk keeps the results in directory order and costs
        // no synchronization; parallelism only pays once a tree is big enough
        // for the walk itself to dominate, and the threshold is well past what a
        // script's own per-path work would notice. `ignore` offers
        // `build_parallel` if that changes.
        let mut hits: Vec<Hit> = Vec::new();
        let walk = builder.build();
        for entry in walk {
            let Ok(entry) = entry else { continue };
            let depth_of = entry.depth();
            // Depth 0 is the start directory itself, which is the frame of the
            // search rather than a result in it -- `fd` does not list it either.
            if depth_of == 0 {
                continue;
            }
            let path = entry.path();
            let Some(rendered) = render(path) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy();

            // Excludes are not re-checked here: `filter_entry` above already
            // dropped every excluded entry *and* its subtree, and the two
            // checks would disagree on a non-`.` start directory (one sees the
            // descent, the other the rendered path). One place decides.
            if !depth.contains(depth_of) {
                continue;
            }
            if !kinds.is_empty() {
                let ft = entry.file_type();
                let is_link = ft.is_some_and(|t| t.is_symlink());
                // A link counts as *what it points at*, so `:files()` keeps a
                // link to a file and `:dirs()` a link to a directory -- the
                // entry still being reported under its own name. Without this
                // an unfollowed link has file type "symlink" and would fall out
                // of both, making `:files()` quietly narrower than no filter at
                // all. `:type("symlink")` is how a script asks for links
                // themselves, so both readings stay available.
                let resolved = if is_link {
                    std::fs::metadata(path).ok().map(|m| m.file_type())
                } else {
                    ft
                };
                let matches_kind = kinds.iter().any(|k| match k {
                    Kind::File => resolved.is_some_and(|t| t.is_file()),
                    Kind::Dir => resolved.is_some_and(|t| t.is_dir()),
                    Kind::Symlink => is_link,
                });
                if !matches_kind {
                    continue;
                }
            }
            if !clauses.iter().all(|c| c.is_match(&rendered, &name)) {
                continue;
            }

            hits.push(Hit {
                path: rendered,
                real: path.to_path_buf(),
            });

            // Nothing below can reorder an unsorted result, so one hit is the
            // whole answer and the rest of the tree need not be read.
            if need == Need::One {
                return Ok(hits);
            }
            // Likewise a limit with no sort: the first `n` in walk order are
            // the answer.
            if sort.is_none() && limit.is_some_and(|n| hits.len() >= n) {
                break;
            }
        }

        sort_hits(&mut hits, sort);
        if let Some(n) = limit {
            hits.truncate(n);
        }
        Ok(hits)
    }
}

/// Order the hits. Path order is the default and needs no `stat`; the metadata
/// keys read it once per entry, treating an unreadable entry as zero rather than
/// failing the whole search for one file that vanished mid-walk.
fn sort_hits(hits: &mut [Hit], sort: Option<SortKey>) {
    match sort {
        None | Some(SortKey::Path) => hits.sort_by(|a, b| a.path.cmp(&b.path)),
        Some(SortKey::Name) => hits.sort_by(|a, b| {
            let an = Path::new(&a.path)
                .file_name()
                .unwrap_or_default()
                .to_owned();
            let bn = Path::new(&b.path)
                .file_name()
                .unwrap_or_default()
                .to_owned();
            an.cmp(&bn).then_with(|| a.path.cmp(&b.path))
        }),
        Some(SortKey::Size) => hits.sort_by(|a, b| {
            let size = |h: &Hit| std::fs::metadata(&h.real).map(|m| m.len()).unwrap_or(0);
            size(a).cmp(&size(b)).then_with(|| a.path.cmp(&b.path))
        }),
        Some(SortKey::Modified) => hits.sort_by(|a, b| {
            let mtime = |h: &Hit| std::fs::metadata(&h.real).and_then(|m| m.modified()).ok();
            // Newest first, with the path breaking ties so the order is total
            // and two runs over an unchanged tree agree.
            match (mtime(a), mtime(b)) {
                (Some(x), Some(y)) => y.cmp(&x).then_with(|| a.path.cmp(&b.path)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => a.path.cmp(&b.path),
            }
        }),
    }
}

/// Parse the `:depth`/`:min_depth`/`:max_depth` argument.
pub fn depth_arg(n: i64) -> Result<usize> {
    usize::try_from(n)
        .map_err(|_| mlua::Error::runtime(format!("depth must be non-negative, got {n}")))
}

/// One matching line, as `:grep()` reports it.
pub struct GrepMatch {
    /// The file, spelled the way every other finder result is.
    pub path: String,
    /// 1-based line number.
    pub line: u64,
    /// The line's text, with the trailing newline removed. Lossy-decoded: a
    /// matched line is being handed to a script that will print or pattern it,
    /// and `nil` for "valid regex match, unrepresentable text" is a worse answer
    /// than the replacement character.
    pub text: String,
    /// The capture groups, when the pattern has any: `1..n` positionally, plus
    /// named groups under their names. Group 0 is *not* included -- it is the
    /// whole match, and a script that wants it has `text` alongside. Empty when
    /// the pattern is group-less, so `#m.captures == 0` distinguishes the cases.
    pub captures: Vec<(String, String)>,
}

/// What `grep` still needs, so a search can stop reading.
#[derive(Clone, Copy, PartialEq)]
enum GrepNeed {
    /// Every matching line (`:grep`), up to a match limit.
    Lines,
    /// One match per file is enough (`:grep_files`) -- the file is the answer,
    /// so the rest of it goes unread.
    FirstPerFile,
}

/// Search `hits` for `pattern`, in walk order.
///
/// `limit` bounds **matches**, not files, and is what stops the search: with a
/// `:limit(n)` in the chain a `:grep()` reads only as far as the nth match, so
/// the tail of a large tree is never opened. That is the one place `limit`
/// changes meaning between terminals, and it is the reading a script wants --
/// "show me ten TODOs" is about TODOs, and "the first ten files, however many
/// TODOs they hold" is not a question anyone asks.
fn grep(
    hits: &[Hit],
    pattern: &str,
    need: GrepNeed,
    limit: Option<usize>,
) -> Result<Vec<GrepMatch>> {
    use grep_regex::RegexMatcher;
    use grep_searcher::sinks::UTF8;
    use grep_searcher::{BinaryDetection, SearcherBuilder};

    let matcher = RegexMatcher::new_line_matcher(pattern)
        .map_err(|e| mlua::Error::runtime(format!("grep: invalid pattern {pattern:?}: {e}")))?;

    // The searcher decides *which lines* match; the capture groups come from a
    // second `regex::Regex` over those lines alone. Two compilations of one
    // pattern is not a dialect seam -- `grep-regex` is a `regex-automata`
    // matcher built from `regex-syntax`, so both agree on what the pattern means
    // -- and it buys the piece `grep-matcher` has no API for: that trait maps a
    // group *name to its index*, never the reverse, so named groups cannot be
    // enumerated through it at all. Compiled once here, not per matching line.
    // `None` when the pattern is group-less, which is also the flag for "build
    // no captures table".
    let groups = match regex::Regex::new(pattern) {
        Ok(re) if re.captures_len() > 1 => Some(re),
        // An `Err` here cannot be a syntax error -- `RegexMatcher` accepted the
        // same pattern above -- so it is a size/complexity limit the two crates
        // configure differently. Losing the groups beats failing a search whose
        // lines matched fine.
        _ => None,
    };

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        // A binary file has no lines worth reporting, and a match inside one is
        // almost always a false positive on compressed bytes. `rg` quits the
        // file at the first NUL; so do we, rather than emitting a "binary file
        // matches" marker a script would have to filter.
        .binary_detection(BinaryDetection::quit(0))
        .build();

    let mut out: Vec<GrepMatch> = Vec::new();
    for hit in hits {
        // A limit is checked between files as well as inside the sink: reaching
        // it means the remaining files are never opened at all.
        if limit.is_some_and(|n| out.len() >= n) {
            break;
        }
        // Only regular files are searchable. A directory in the hit list is not
        // an error -- an unfiltered finder reports both -- so it is skipped
        // rather than failing the run, and a script that wants the distinction
        // spells `:files()`.
        if !hit.real.is_file() {
            continue;
        }

        let mut file_matches: Vec<GrepMatch> = Vec::new();
        let remaining = limit.map(|n| n.saturating_sub(out.len()));
        let path = hit.path.clone();
        let result = searcher.search_path(
            &matcher,
            &hit.real,
            UTF8(|line_number, text| {
                file_matches.push(GrepMatch {
                    path: path.clone(),
                    line: line_number,
                    text: text.trim_end_matches(['\n', '\r']).to_string(),
                    captures: Vec::new(),
                });
                // `false` ends this file's search. Both stopping conditions are
                // per-file: the caller's limit is what the outer loop enforces.
                if need == GrepNeed::FirstPerFile {
                    return Ok(false);
                }
                Ok(remaining.is_none_or(|n| file_matches.len() < n))
            }),
        );
        // An unreadable file is skipped, not fatal: a search over a tree is a
        // survey, and one file whose permissions changed (or that vanished
        // mid-walk) must not lose the other results. The same judgement
        // `sort_hits` makes about an unreadable `stat`.
        if result.is_err() {
            continue;
        }

        if let Some(re) = &groups {
            for m in &mut file_matches {
                m.captures = capture_pairs(re, &m.text);
            }
        }
        out.append(&mut file_matches);
    }

    if let Some(n) = limit {
        out.truncate(n);
    }
    Ok(out)
}

/// Pull the capture groups out of one matched line.
///
/// A second pass over a single line, run only for a pattern that has groups and
/// only for lines the searcher already matched. The alternative is asking the
/// sink for captures on every line of every file, which pays the cost on the
/// group-less case too.
///
/// Only the *first* match in the line is reported: a `GrepMatch` is one line and
/// carries one `captures` table, so a line matching twice would have to either
/// drop the second or become two results claiming the same line number. The
/// full text is there for a script that needs the rest.
fn capture_pairs(re: &regex::Regex, text: &str) -> Vec<(String, String)> {
    let Some(caps) = re.captures(text) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    // Group 0 (the whole match) is kept, so the shape is `re.captures`'s: the
    // line's `text` is the whole *line*, group 0 just the match within it.
    for (i, group) in caps.iter().enumerate() {
        // A group that did not participate is omitted rather than empty-stringed
        // -- the distinction `re.captures` also keeps.
        let Some(m) = group else { continue };
        pairs.push((i.to_string(), m.as_str().to_string()));
    }
    for name in re.capture_names().flatten() {
        if let Some(m) = caps.name(name) {
            pairs.push((name.to_string(), m.as_str().to_string()));
        }
    }
    pairs
}

/// Install the chainable method surface.
///
/// One implementation for both `fs` variants: everything that differs between
/// the rooted and host filesystems is behind [`Source`], so the script-visible
/// API cannot drift between them. Every filter returns the handle, so the chain
/// reads left to right and ends at a terminal.
///
/// `T` is the host's newtype around [`Finder`] — each `fs` module owns one so
/// it can implement `UserData` locally — and `project` reaches the finder
/// inside it.
pub fn add_methods<T, S, M>(methods: &mut M, project: fn(&T) -> &Finder<S>)
where
    T: Clone + mlua::UserData + 'static,
    S: Source + 'static,
    M: mlua::UserDataMethods<T>,
{
    add_common_methods(methods, project);
}

/// Install [`add_methods`] plus the host-only methods: `:follow()` and the two
/// grep terminals.
///
/// Following links is host-only, and absent rather than refusing on the rooted
/// finder — the same "the function is not there" boundary the rest of the
/// stdlib draws, so `if fs.cwd then` and a missing `:follow` read alike. Under
/// a root the method could not mean what it says: a link leaving the root is
/// dropped when the path is rendered, so `:follow()` would be a switch that
/// changed nothing about the answer.
///
/// `:grep()`/`:grep_files()` are here for a different reason, and a temporary
/// one: they are the only terminals that *open* a matched file, so under a root
/// they would need the containment check to cover reads as well as paths.
/// [`Source::renderer`] settles which paths may be named, not which bytes may be
/// read, and the two are the same question only as long as nothing follows a
/// rendered path back to disk. Wiring that up is the work of exposing grep on
/// `ied`; until then the rooted finder simply does not have it.
pub fn add_methods_with_follow<T, S, M>(methods: &mut M, project: fn(&T) -> &Finder<S>)
where
    T: Clone + mlua::UserData + 'static,
    S: Source + 'static,
    M: mlua::UserDataMethods<T>,
{
    add_common_methods(methods, project);
    methods.add_method("follow", move |_, t, ()| {
        project(t).set_follow(true);
        Ok(t.clone())
    });

    // Content terminals. These are the only finder methods that read a file, so
    // they are also the only ones whose cost scales with file *size* rather than
    // entry count -- which is the whole point: the alternative is `fs.read` in a
    // Luau loop feeding `re.find`, paying a string copy per file and a regex
    // compile per call.
    //
    // `:grep(pattern) -> {{path, line, text, captures}}`
    methods.add_method("grep", move |lua, t, pattern: String| {
        let matches = project(t).grep(&pattern)?;
        let out = lua.create_table_with_capacity(matches.len(), 0)?;
        for m in matches {
            let row = lua.create_table()?;
            row.set("path", m.path)?;
            row.set("line", m.line)?;
            row.set("text", m.text)?;
            // Always present, empty for a group-less pattern: a script indexing
            // `m.captures[1]` on the wrong pattern gets nil rather than an
            // error about indexing nil.
            let caps = lua.create_table()?;
            for (key, value) in m.captures {
                // A numbered group is set under an integer key so `caps[1]`
                // works and `#caps` counts -- Luau's `"1"` and `1` are distinct
                // keys, and the positional reading is the common one.
                match key.parse::<i64>() {
                    Ok(i) => caps.set(i, value)?,
                    Err(_) => caps.set(key, value)?,
                }
            }
            row.set("captures", caps)?;
            out.push(row)?;
        }
        Ok(out)
    });

    // `:grep_files(pattern) -> {path}`
    methods.add_method("grep_files", move |_, t, pattern: String| {
        project(t).grep_files(&pattern)
    });
}

/// The filters and terminals every finder has.
fn add_common_methods<T, S, M>(methods: &mut M, project: fn(&T) -> &Finder<S>)
where
    T: Clone + mlua::UserData + 'static,
    S: Source + 'static,
    M: mlua::UserDataMethods<T>,
{
    // Filters. Each takes one or more patterns; the alternatives within a call
    // OR, and repeated calls AND.
    methods.add_method("name", move |_, t, patterns: mlua::Variadic<String>| {
        project(t).push_clause(&patterns, Match::Name)?;
        Ok(t.clone())
    });
    methods.add_method("path", move |_, t, patterns: mlua::Variadic<String>| {
        project(t).push_clause(&patterns, Match::Path)?;
        Ok(t.clone())
    });
    // `:ext("rs")` is `:name("*.rs")` with the dot handled, and accepts it
    // either way -- a script holding an extension from `path.extension` has the
    // bare form, one typing a literal tends to write the dot. `""` is the
    // extension such a script gets back for `Makefile`, so it means that here
    // too, and is the one alternative with no glob spelling.
    methods.add_method("ext", move |_, t, exts: mlua::Variadic<String>| {
        project(t).push_ext_clause(&exts)?;
        Ok(t.clone())
    });

    // Exclusions prune: a directory matching one is never descended into.
    methods.add_method("exclude", move |_, t, patterns: mlua::Variadic<String>| {
        // Auto-targeting is deliberate *here* and nowhere else: an exclude is
        // the one filter where both readings are routinely wanted at once --
        // `"node_modules"` names a directory, `"**/target/**"` names a region --
        // and unlike the old seed glob there is no second method it could be
        // confused with.
        for p in patterns.iter() {
            let target = if p.contains('/') {
                Match::Path
            } else {
                Match::Name
            };
            project(t).push_exclude(std::slice::from_ref(p), target)?;
        }
        Ok(t.clone())
    });

    // Type filters. Repeating widens, so `:files():dirs()` is "either".
    methods.add_method("files", move |_, t, ()| {
        project(t).push_kind(Kind::File);
        Ok(t.clone())
    });
    methods.add_method("dirs", move |_, t, ()| {
        project(t).push_kind(Kind::Dir);
        Ok(t.clone())
    });
    methods.add_method("type", move |_, t, kinds: mlua::Variadic<String>| {
        for k in kinds.iter() {
            project(t).push_kind(Kind::parse(k)?);
        }
        Ok(t.clone())
    });

    // Depth. Three methods with one meaning each, rather than one method whose
    // meaning changed with its arity.
    methods.add_method("depth", move |_, t, (min, max): (i64, Option<i64>)| {
        let min = depth_arg(min)?;
        let max = match max {
            Some(m) => Some(depth_arg(m)?),
            // `:depth(n)` alone is the exact depth, as `fd --exact-depth`.
            None => Some(min),
        };
        project(t).set_depth(DepthBound {
            min: Some(min),
            max,
        });
        Ok(t.clone())
    });
    methods.add_method("min_depth", move |_, t, n: i64| {
        let n = depth_arg(n)?;
        let f = project(t);
        let mut d = f.state.borrow().depth;
        d.min = Some(n);
        f.set_depth(d);
        Ok(t.clone())
    });
    methods.add_method("max_depth", move |_, t, n: i64| {
        let n = depth_arg(n)?;
        let f = project(t);
        let mut d = f.state.borrow().depth;
        d.max = Some(n);
        f.set_depth(d);
        Ok(t.clone())
    });

    // Restriction levels, named for what they add rather than for the flag
    // that adds it.
    methods.add_method("hidden", move |_, t, ()| {
        project(t).state.borrow_mut().restrictions.skip_hidden = false;
        Ok(t.clone())
    });
    methods.add_method("no_ignore", move |_, t, ()| {
        project(t).state.borrow_mut().restrictions.use_ignore_files = false;
        Ok(t.clone())
    });
    methods.add_method("unrestricted", move |_, t, ()| {
        let f = project(t);
        let mut state = f.state.borrow_mut();
        state.restrictions.skip_hidden = false;
        state.restrictions.use_ignore_files = false;
        drop(state);
        Ok(t.clone())
    });

    // Ordering.
    methods.add_method("sort", move |_, t, key: String| {
        project(t).set_sort(SortKey::parse(&key)?);
        Ok(t.clone())
    });
    methods.add_method("limit", move |_, t, n: i64| {
        let n = usize::try_from(n)
            .map_err(|_| mlua::Error::runtime(format!("limit must be non-negative, got {n}")))?;
        project(t).set_limit(n);
        Ok(t.clone())
    });

    // Terminals.
    methods.add_method("find", move |_, t, ()| {
        Ok(project(t)
            .find()?
            .into_iter()
            .map(|h| h.path)
            .collect::<Vec<_>>())
    });
    methods.add_method("count", move |_, t, ()| project(t).count());
    methods.add_method("any", move |_, t, ()| project(t).any());
}
