//! Hunk- and line-level staging.
//!
//! Instead of generating textual patches (fragile: path quoting, CRLF,
//! "\ No newline" bookkeeping), we rebuild the desired *index blob* in memory
//! and write it with `Index::add_frombuffer`:
//!
//! * **Stage** works on the `index → workdir` diff. Base = index content (old
//!   side). Selected deletions are dropped, selected additions inserted.
//! * **Unstage** works on the `HEAD → index` diff. Base = index content (new
//!   side). Selected additions are dropped, selected deletions re-inserted.
//!
//! Both reduce to one rule: *base-side lines that are selected disappear,
//! foreign-side lines that are selected appear.* Every base line we touch is
//! compared byte-for-byte with what the UI saw; any drift → `AppError::Stale`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use git2::Repository;

use super::diff::{self, DiffParams, RawHunk};
use crate::error::{AppError, AppResult};
use crate::models::{
    DiffLineKind, DiffTarget, FileStatus, HunkSelection, StageAction, StageRequest,
};
use crate::paths::git_relative;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BaseSide {
    Old,
    New,
}

pub fn apply(repo: &Repository, req: &StageRequest) -> AppResult<()> {
    // Validates the path (no traversal) before it reaches libgit2.
    let rel = git_relative(&req.path)?;
    if req.hunks.is_empty() {
        return whole_file(repo, &req.path, &rel, req.action);
    }
    partial(repo, req)
}

fn whole_file(repo: &Repository, git_path: &str, rel: &Path, action: StageAction) -> AppResult<()> {
    let mut index = repo.index()?;
    match action {
        StageAction::Stage => {
            let workdir = repo.workdir().ok_or_else(|| {
                AppError::Unsupported("bare repositories have no working tree".into())
            })?;
            if workdir.join(rel).symlink_metadata().is_ok() {
                index.add_path(rel)?;
            } else {
                index.remove_path(rel)?;
            }
            index.write()?;
        }
        StageAction::Unstage => match repo.head() {
            Ok(head) => {
                let commit = head.peel_to_commit()?;
                // Resets only this path's index entry to HEAD (removes it if
                // it is not in HEAD). The working tree is untouched.
                repo.reset_default(Some(commit.as_object()), [git_path])?;
            }
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
                index.remove_path(rel)?;
                index.write()?;
            }
            Err(e) => return Err(e.into()),
        },
    }
    Ok(())
}

fn partial(repo: &Repository, req: &StageRequest) -> AppResult<()> {
    let (target, side) = match req.action {
        StageAction::Stage => (DiffTarget::WorkingTree, BaseSide::Old),
        StageAction::Unstage => (DiffTarget::Index, BaseSide::New),
    };
    let params = DiffParams {
        context_lines: req.context_lines,
        ignore_whitespace: false,
    };
    let diff = diff::compute(repo, &target, params, Some(&req.path))?;
    if diff.deltas().len() != 1 {
        return Err(AppError::Stale(format!(
            "{} has no pending changes",
            req.path
        )));
    }
    let (file, raw_hunks) = diff::file_at(&diff, 0)?;

    if file.status != FileStatus::Modified {
        return Err(AppError::Unsupported(format!(
            "line staging needs a modified file; stage {} as a whole",
            req.path
        )));
    }
    if file.is_binary || file.truncated {
        return Err(AppError::Unsupported(
            "line staging is not available for binary or truncated diffs".into(),
        ));
    }

    let selection = selection_map(&req.hunks, &raw_hunks)?;

    let mut index = repo.index()?;
    let entry = index
        .get_path(Path::new(&req.path), 0)
        .ok_or_else(|| AppError::Stale(format!("{} is not in the index", req.path)))?;

    // The index blob must be the base the diff was computed against.
    let delta = diff
        .get_delta(0)
        .ok_or_else(|| AppError::Internal("delta vanished".into()))?;
    let base_id = match side {
        BaseSide::Old => delta.old_file().id(),
        BaseSide::New => delta.new_file().id(),
    };
    if base_id != entry.id {
        return Err(AppError::Stale("the index changed while staging".into()));
    }

    let base = repo.find_blob(entry.id)?;
    let new_content = rebuild(base.content(), &raw_hunks, &selection, side)?;

    index.add_frombuffer(&entry, &new_content)?;
    index.write()?;
    Ok(())
}

/// hunk index → `None` (whole hunk) or the selected line indices.
type Selection = HashMap<usize, Option<HashSet<usize>>>;

fn selection_map(sel: &[HunkSelection], hunks: &[RawHunk]) -> AppResult<Selection> {
    let mut map = Selection::new();
    for s in sel {
        let hunk = hunks
            .get(s.hunk_index)
            .ok_or_else(|| AppError::Stale(format!("hunk {} no longer exists", s.hunk_index)))?;
        if hunk.header.trim() != s.header.trim() {
            return Err(AppError::Stale(format!(
                "hunk {} changed ({} → {})",
                s.hunk_index, s.header, hunk.header
            )));
        }
        let lines = match &s.lines {
            None => None,
            Some(v) => {
                if let Some(bad) = v.iter().find(|&&i| i >= hunk.lines.len()) {
                    return Err(AppError::InvalidInput(format!(
                        "line {bad} out of range in hunk {}",
                        s.hunk_index
                    )));
                }
                Some(v.iter().copied().collect())
            }
        };
        map.insert(s.hunk_index, lines);
    }
    Ok(map)
}

fn rebuild(
    base: &[u8],
    hunks: &[RawHunk],
    selection: &Selection,
    side: BaseSide,
) -> AppResult<Vec<u8>> {
    let base_lines = split_lines(base);
    let mut out: Vec<u8> = Vec::with_capacity(base.len() + 256);
    let mut consumed = 0usize;

    // Ensures we never glue two lines together when a line that lacked a
    // final newline stops being the last one.
    fn push_line(out: &mut Vec<u8>, line: &[u8]) {
        if out.last().is_some_and(|&b| b != b'\n') {
            out.push(b'\n');
        }
        out.extend_from_slice(line);
    }

    for (hi, hunk) in hunks.iter().enumerate() {
        let Some(sel) = selection.get(&hi) else {
            continue; // untouched hunk: base lines are copied verbatim below
        };
        let (start, count) = match side {
            BaseSide::Old => (hunk.old_start, hunk.old_lines),
            BaseSide::New => (hunk.new_start, hunk.new_lines),
        };
        // `@@ -5,0 ...` means "after line 5"; `@@ -5,2 ...` starts at line 5.
        let skip_to = if count == 0 {
            start as usize
        } else {
            (start as usize).saturating_sub(1)
        };
        if skip_to < consumed || skip_to > base_lines.len() {
            return Err(AppError::Stale(
                "hunk positions do not match the index".into(),
            ));
        }
        for line in &base_lines[consumed..skip_to] {
            push_line(&mut out, line);
        }
        consumed = skip_to;

        for (li, line) in hunk.lines.iter().enumerate() {
            let selected = match sel {
                None => true,
                Some(set) => set.contains(&li),
            };
            let in_base = matches!(
                (line.kind, side),
                (DiffLineKind::Context, _)
                    | (DiffLineKind::Deletion, BaseSide::Old)
                    | (DiffLineKind::Addition, BaseSide::New)
            );

            if in_base {
                let base_line = base_lines.get(consumed).ok_or_else(|| {
                    AppError::Stale("diff extends past the end of the index file".into())
                })?;
                if *base_line != line.bytes.as_slice() {
                    return Err(AppError::Stale(
                        "index content differs from the displayed diff".into(),
                    ));
                }
                consumed += 1;
                let drop = selected && line.kind != DiffLineKind::Context;
                if !drop {
                    push_line(&mut out, base_line);
                }
            } else if selected {
                push_line(&mut out, &line.bytes);
            }
        }
    }

    for line in &base_lines[consumed..] {
        push_line(&mut out, line);
    }
    Ok(out)
}

/// Splits after every `\n`, keeping terminators; a final line without `\n`
/// is kept as-is.
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            lines.push(&content[start..=i]);
            start = i + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}
