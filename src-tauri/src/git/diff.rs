//! Diff computation: git2 deltas → structured hunks for the UI.
//!
//! Two representations are produced from the same `git2::Patch`:
//! * [`FileDiff`] (lossy UTF-8, sent to the UI);
//! * [`RawHunk`] (exact bytes, used only by `stage.rs`).
//!
//! Both are built by the same walk, so hunk and line indices always agree —
//! the UI addresses lines by index and the backend re-derives the bytes.

use git2::{Delta, Diff, DiffFindOptions, DiffOptions, Patch, Repository};

use crate::error::{AppError, AppResult};
use crate::models::{DiffLine, DiffLineKind, DiffResult, DiffTarget, FileDiff, FileStatus, Hunk};

/// Per-file line budget; beyond it the file is marked `truncated` and only
/// whole-file staging is offered. Keeps IPC payloads and the DOM bounded.
pub const MAX_LINES_PER_FILE: usize = 20_000;
/// "Show full file" context. libgit2 clamps to the file length.
pub const FULL_FILE_CONTEXT: u32 = 1_000_000;

#[derive(Debug, Clone, Copy)]
pub struct DiffParams {
    pub context_lines: u32,
    pub ignore_whitespace: bool,
}

impl Default for DiffParams {
    fn default() -> Self {
        Self {
            context_lines: 3,
            ignore_whitespace: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawLine {
    pub kind: DiffLineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    /// Exact bytes, including the trailing `\n` when the line has one.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RawHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<RawLine>,
}

/// Builds the libgit2 diff for a target. `pathspec` restricts it to one exact
/// path (no glob expansion).
pub fn compute<'r>(
    repo: &'r Repository,
    target: &DiffTarget,
    params: DiffParams,
    pathspec: Option<&str>,
) -> AppResult<Diff<'r>> {
    let mut opts = DiffOptions::new();
    opts.context_lines(params.context_lines)
        .interhunk_lines(0)
        .include_typechange(true)
        .ignore_whitespace(params.ignore_whitespace);
    if let Some(p) = pathspec {
        opts.pathspec(p).disable_pathspec_match(true);
    }

    let mut diff = match target {
        DiffTarget::WorkingTree => {
            opts.include_untracked(true)
                .recurse_untracked_dirs(true)
                .show_untracked_content(true);
            repo.diff_index_to_workdir(None, Some(&mut opts))?
        }
        DiffTarget::Index => {
            let head_tree = match repo.head() {
                Ok(head) => Some(head.peel_to_tree()?),
                // Unborn branch: everything in the index is "added".
                Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
                Err(e) => return Err(e.into()),
            };
            repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))?
        }
        DiffTarget::Commit { id } => {
            let oid = git2::Oid::from_str(id)
                .map_err(|_| AppError::InvalidInput(format!("invalid commit id {id:?}")))?;
            let commit = repo.find_commit(oid)?;
            let new_tree = commit.tree()?;
            let old_tree = match commit.parent_count() {
                0 => None,
                _ => Some(commit.parent(0)?.tree()?),
            };
            repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))?
        }
    };

    // Rename detection for committed/staged changes (the working tree diff
    // keeps plain add/delete so line staging maps 1:1 onto index entries).
    if !matches!(target, DiffTarget::WorkingTree) {
        let mut find = DiffFindOptions::new();
        find.renames(true).copies(false);
        diff.find_similar(Some(&mut find))?;
    }
    Ok(diff)
}

pub fn to_result(diff: &Diff<'_>, target: DiffTarget, params: DiffParams) -> AppResult<DiffResult> {
    let mut files = Vec::with_capacity(diff.deltas().len());
    for idx in 0..diff.deltas().len() {
        let (file, _) = file_at(diff, idx)?;
        files.push(file);
    }
    Ok(DiffResult {
        target,
        context_lines: params.context_lines,
        files,
    })
}

/// Converts one delta into both the UI model and the raw model.
pub fn file_at(diff: &Diff<'_>, idx: usize) -> AppResult<(FileDiff, Vec<RawHunk>)> {
    let delta = diff
        .get_delta(idx)
        .ok_or_else(|| AppError::Internal(format!("delta {idx} out of range")))?;
    let status = map_status(delta.status());
    let old_path = delta.old_file().path().map(path_string);
    let new_path = delta.new_file().path().map(path_string);

    let mut file = FileDiff {
        old_path: old_path
            .filter(|_| status != FileStatus::Added && status != FileStatus::Untracked),
        new_path: new_path.filter(|_| status != FileStatus::Deleted),
        status,
        is_binary: delta.flags().is_binary(),
        truncated: false,
        additions: 0,
        deletions: 0,
        hunks: Vec::new(),
    };
    let mut raw_hunks = Vec::new();

    let patch = match Patch::from_diff(diff, idx)? {
        Some(p) => p,
        // Binary or unchanged content: no textual patch.
        None => {
            file.is_binary = true;
            return Ok((file, raw_hunks));
        }
    };
    if patch.delta().flags().is_binary() {
        file.is_binary = true;
        return Ok((file, raw_hunks));
    }

    let mut budget = MAX_LINES_PER_FILE;
    for h in 0..patch.num_hunks() {
        let (hunk, line_count) = patch.hunk(h)?;
        let header = String::from_utf8_lossy(hunk.header()).trim_end().to_owned();
        let mut ui = Hunk {
            header: header.clone(),
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            lines: Vec::with_capacity(line_count),
        };
        let mut raw = RawHunk {
            header,
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            lines: Vec::with_capacity(line_count),
        };

        for l in 0..line_count {
            let line = patch.line_in_hunk(h, l)?;
            let kind = match line.origin() {
                ' ' => DiffLineKind::Context,
                '+' => DiffLineKind::Addition,
                '-' => DiffLineKind::Deletion,
                // '=', '>', '<': "\ No newline at end of file" markers that
                // annotate the previous line rather than being lines.
                '=' | '>' | '<' => {
                    if let Some(prev) = ui.lines.last_mut() {
                        prev.no_newline = true;
                    }
                    continue;
                }
                _ => continue,
            };
            match kind {
                DiffLineKind::Addition => file.additions += 1,
                DiffLineKind::Deletion => file.deletions += 1,
                DiffLineKind::Context => {}
            }
            if budget == 0 {
                file.truncated = true;
                continue; // keep counting additions/deletions
            }
            budget -= 1;

            let bytes = line.content();
            ui.lines.push(DiffLine {
                kind,
                old_lineno: line.old_lineno(),
                new_lineno: line.new_lineno(),
                content: display_text(bytes),
                no_newline: false,
            });
            raw.lines.push(RawLine {
                kind,
                old_lineno: line.old_lineno(),
                new_lineno: line.new_lineno(),
                bytes: bytes.to_vec(),
            });
        }
        file.hunks.push(ui);
        raw_hunks.push(raw);
    }
    Ok((file, raw_hunks))
}

fn display_text(bytes: &[u8]) -> String {
    let trimmed = bytes
        .strip_suffix(b"\n")
        .map(|b| b.strip_suffix(b"\r").unwrap_or(b))
        .unwrap_or(bytes);
    String::from_utf8_lossy(trimmed).into_owned()
}

/// git paths are `/`-separated on every OS; keep them that way for the UI
/// and for pathspecs.
fn path_string(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

fn map_status(d: Delta) -> FileStatus {
    match d {
        Delta::Added => FileStatus::Added,
        Delta::Deleted => FileStatus::Deleted,
        Delta::Modified => FileStatus::Modified,
        Delta::Renamed => FileStatus::Renamed,
        Delta::Copied => FileStatus::Copied,
        Delta::Typechange => FileStatus::TypeChange,
        Delta::Untracked => FileStatus::Untracked,
        Delta::Conflicted => FileStatus::Conflicted,
        Delta::Unmodified | Delta::Ignored | Delta::Unreadable => FileStatus::Unmodified,
    }
}
