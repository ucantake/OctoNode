//! Commit DAG layout (lane assignment).
//!
//! The whole layout is computed once per repository state on the Rust side and
//! cached; the UI requests windows of rows (`offset`, `limit`) while virtually
//! scrolling. Layout is O(commits × active lanes); in practice it is dominated
//! by libgit2 object decoding (~15 µs/commit): rust-lang/cargo (24k commits,
//! 21 lanes) lays out in ~0.4 s in release builds, once per ref change.
//!
//! ## Model
//! `lanes[k]` holds the commit id that lane `k` is waiting for. Rows are
//! processed in topological order (children before parents):
//!
//! 1. The commit takes the leftmost lane that expects it, or a free lane.
//!    Other lanes expecting it converge into it and are freed.
//! 2. The first parent inherits the commit's lane (keeps mainline straight);
//!    if that parent already has a lane, ours merges into it.
//! 3. Each further parent joins its existing lane or opens a new one.
//!
//! Each row stores the *incoming* segments from the previous row center to its
//! own center, so any window of rows can be drawn independently.

use std::collections::HashMap;

use git2::{BranchType, Oid, Repository, Sort};

use crate::error::AppResult;
use crate::models::{CommitNode, GraphEdge, GraphPage, RefBadge, RefKind};

/// Palette size on the frontend; color indices are taken modulo this.
const PALETTE_SIZE: u8 = 12;
/// Lanes beyond this are clamped visually (pathological octopus histories).
const MAX_LANES: usize = u16::MAX as usize;
const SHORT_ID_LEN: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct GraphOptions {
    pub max_commits: usize,
    pub include_remotes: bool,
    pub include_tags: bool,
}

impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            max_commits: 200_000,
            include_remotes: true,
            include_tags: true,
        }
    }
}

/// Fully laid-out history, cached in `AppState`.
#[derive(Debug)]
pub struct CommitGraph {
    /// Changes whenever any ref moves; used for cache invalidation.
    pub fingerprint: u64,
    pub rows: Vec<CommitNode>,
    pub max_lanes: u16,
    pub truncated: bool,
}

impl CommitGraph {
    pub fn page(&self, offset: usize, limit: usize) -> GraphPage {
        let start = offset.min(self.rows.len());
        let end = start.saturating_add(limit).min(self.rows.len());
        GraphPage {
            total: self.rows.len(),
            offset: start,
            max_lanes: self.max_lanes,
            truncated: self.truncated,
            rows: self.rows[start..end].to_vec(),
        }
    }
}

#[derive(Clone, Copy)]
struct Lane {
    /// Commit this lane is waiting for.
    target: Oid,
    color: u8,
    /// Column at the previous row the segment into this lane starts from.
    from: u16,
}

/// Cheap hash over all refs (+HEAD). Equal fingerprint ⇒ identical graph.
pub fn fingerprint(repo: &Repository) -> AppResult<u64> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Ok(head) = repo.head() {
        head.name().hash(&mut h);
        head.target().map(|o| o.as_bytes().to_vec()).hash(&mut h);
    }
    for r in repo.references()? {
        let r = r?;
        r.name_bytes().hash(&mut h);
        r.target().map(|o| o.as_bytes().to_vec()).hash(&mut h);
    }
    Ok(h.finish())
}

pub fn build_graph(repo: &Repository, opts: GraphOptions) -> AppResult<CommitGraph> {
    let fingerprint = fingerprint(repo)?;
    let refs = collect_refs(repo, opts)?;

    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    let mut pushed_any = false;
    for oid in refs.keys() {
        // Tags may point at trees/blobs; only commits are walkable.
        if repo.find_commit(*oid).is_ok() {
            walk.push(*oid)?;
            pushed_any = true;
        }
    }
    if !pushed_any {
        // Empty / unborn repository.
        return Ok(CommitGraph {
            fingerprint,
            rows: Vec::new(),
            max_lanes: 0,
            truncated: false,
        });
    }

    let mut lanes: Vec<Option<Lane>> = Vec::new();
    // Bends from the previous commit into lanes that already existed:
    // (from column, lane index, color). Consumed by the next row.
    let mut pending_bends: Vec<(u16, usize, u8)> = Vec::new();
    let mut rows = Vec::new();
    let mut max_lanes = 0usize;
    let mut next_color: u8 = 0;
    let mut truncated = false;

    let mut alloc_color = || {
        let c = next_color;
        next_color = (next_color + 1) % PALETTE_SIZE;
        c
    };

    for oid in walk {
        if rows.len() >= opts.max_commits {
            truncated = true;
            break;
        }
        let oid = oid?;
        let commit = repo.find_commit(oid)?;

        // --- 1. place the commit ------------------------------------------
        let expecting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_some_and(|l| l.target == oid))
            .map(|(i, _)| i)
            .collect();

        let (col, color) = match expecting.first() {
            Some(&i) => (i, lanes[i].map_or(0, |l| l.color)),
            None => (free_slot(&mut lanes), alloc_color()),
        };
        let col16 = clamp_lane(col);
        let lands_at = |k: usize, lanes: &[Option<Lane>]| -> u16 {
            match lanes.get(k).copied().flatten() {
                Some(l) if l.target == oid => col16,
                _ => clamp_lane(k),
            }
        };

        // Incoming segments: active lanes continue straight down, except the
        // ones converging on this commit, which bend into `col`.
        let mut edges = Vec::with_capacity(lanes.len() + pending_bends.len());
        for (k, lane) in lanes.iter().enumerate() {
            if let Some(lane) = lane {
                edges.push(GraphEdge(lane.from, lands_at(k, &lanes), lane.color));
            }
        }
        for (from, m, c) in pending_bends.drain(..) {
            edges.push(GraphEdge(from, lands_at(m, &lanes), c));
        }
        for &k in &expecting {
            lanes[k] = None;
        }

        // --- 2./3. route parents -------------------------------------------
        let parent_ids: Vec<Oid> = commit.parent_ids().collect();
        let mut routed_here: Vec<usize> = Vec::with_capacity(parent_ids.len());
        for (idx, parent) in parent_ids.iter().enumerate() {
            let tracked = lanes
                .iter()
                .position(|l| l.is_some_and(|l| l.target == *parent));
            match tracked {
                Some(m) => {
                    // Parent already has a lane: draw one bend into it. The
                    // first-parent bend keeps the branch color; extra parents
                    // take the color of the lane they join.
                    let c = if idx == 0 {
                        color
                    } else {
                        lanes[m].map_or(color, |l| l.color)
                    };
                    pending_bends.push((col16, m, c));
                }
                None => {
                    let (slot, c) = if idx == 0 && lanes[col].is_none() {
                        (col, color)
                    } else {
                        (free_slot(&mut lanes), alloc_color())
                    };
                    lanes[slot] = Some(Lane {
                        target: *parent,
                        color: c,
                        from: col16,
                    });
                    routed_here.push(slot);
                }
            }
        }

        // Lanes not routed from this commit continue straight next row.
        for (k, lane) in lanes.iter_mut().enumerate() {
            if let Some(l) = lane {
                if !routed_here.contains(&k) {
                    l.from = clamp_lane(k);
                }
            }
        }
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
        max_lanes = max_lanes.max(lanes.len()).max(col + 1);

        // `Object::short_id` probes the ODB for uniqueness per commit, which
        // dominates layout time on large repos; a fixed 8-char prefix is what
        // the UI shows and the full id is always available.
        let id = oid.to_string();
        let short_id = id[..SHORT_ID_LEN.min(id.len())].to_owned();
        let author = commit.author();

        rows.push(CommitNode {
            id,
            short_id,
            summary: String::from_utf8_lossy(commit.summary_bytes().unwrap_or_default())
                .into_owned(),
            author_name: String::from_utf8_lossy(author.name_bytes()).into_owned(),
            author_email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
            time: author.when().seconds(),
            parents: parent_ids.iter().map(Oid::to_string).collect(),
            lane: col16,
            color,
            edges,
            refs: refs.get(&oid).cloned().unwrap_or_default(),
        });
    }

    Ok(CommitGraph {
        fingerprint,
        rows,
        max_lanes: clamp_lane(max_lanes),
        truncated,
    })
}

fn clamp_lane(i: usize) -> u16 {
    i.min(MAX_LANES - 1) as u16
}

fn free_slot(lanes: &mut Vec<Option<Lane>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

fn collect_refs(repo: &Repository, opts: GraphOptions) -> AppResult<HashMap<Oid, Vec<RefBadge>>> {
    let mut map: HashMap<Oid, Vec<RefBadge>> = HashMap::new();

    let head = repo.head().ok();
    let head_branch = head
        .as_ref()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().map(str::to_owned));
    if let Some(h) = &head {
        if !h.is_branch() {
            if let Some(oid) = h.target() {
                map.entry(oid).or_default().push(RefBadge {
                    name: "HEAD".into(),
                    kind: RefKind::Head,
                    is_head: true,
                });
            }
        }
    }

    let branch_filter = if opts.include_remotes {
        None
    } else {
        Some(BranchType::Local)
    };
    for branch in repo.branches(branch_filter)? {
        let (branch, kind) = branch?;
        let Some(name) = branch.name()?.map(str::to_owned) else {
            continue;
        };
        let Some(oid) = branch.get().target() else {
            // Symbolic remote HEAD (`origin/HEAD`), skip.
            continue;
        };
        if kind == BranchType::Remote && name.ends_with("/HEAD") {
            continue;
        }
        let is_head = kind == BranchType::Local && head_branch.as_deref() == Some(name.as_str());
        map.entry(oid).or_default().push(RefBadge {
            name,
            kind: match kind {
                BranchType::Local => RefKind::LocalBranch,
                BranchType::Remote => RefKind::RemoteBranch,
            },
            is_head,
        });
    }

    if opts.include_tags {
        repo.tag_foreach(|oid, name| {
            let name = String::from_utf8_lossy(name);
            let short = name.strip_prefix("refs/tags/").unwrap_or(&name).to_owned();
            // Peel annotated tags to the commit they point at.
            let target = repo
                .find_object(oid, None)
                .and_then(|o| o.peel_to_commit())
                .map(|c| c.id());
            if let Ok(commit_id) = target {
                map.entry(commit_id).or_default().push(RefBadge {
                    name: short,
                    kind: RefKind::Tag,
                    is_head: false,
                });
            }
            true
        })?;
    }

    // Stable order: HEAD branch first, then locals, remotes, tags.
    for badges in map.values_mut() {
        badges.sort_by_key(|b| {
            let rank = match (b.is_head, b.kind) {
                (true, _) => 0,
                (_, RefKind::Head) => 0,
                (_, RefKind::LocalBranch) => 1,
                (_, RefKind::RemoteBranch) => 2,
                (_, RefKind::Tag) => 3,
            };
            (rank, b.name.clone())
        });
    }
    Ok(map)
}
