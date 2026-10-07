//! Integration tests against real temporary repositories.

use std::fs;
use std::path::Path;

use git2::{Oid, Repository, Signature};
use uuid::Uuid;

use super::graph::{build_graph, GraphOptions};
use super::{diff, stage};
use crate::models::{DiffLineKind, DiffTarget, HunkSelection, StageAction, StageRequest};

fn init() -> (tempfile::TempDir, Repository) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = Repository::init(dir.path()).expect("init");
    (dir, repo)
}

fn commit_file(
    repo: &Repository,
    name: &str,
    content: &str,
    msg: &str,
    parents: &[Oid],
    time: i64,
) -> Oid {
    let root = repo.workdir().expect("workdir");
    fs::write(root.join(name), content).expect("write");
    let mut index = repo.index().expect("index");
    index.add_path(Path::new(name)).expect("add");
    index.write().expect("write index");
    let tree = repo
        .find_tree(index.write_tree().expect("tree"))
        .expect("find tree");
    let parents: Vec<_> = parents
        .iter()
        .map(|p| repo.find_commit(*p).expect("parent"))
        .collect();
    let parent_refs: Vec<_> = parents.iter().collect();
    let s = Signature::new("Tester", "t@example.com", &git2::Time::new(time, 0)).expect("sig");
    repo.commit(None, &s, &s, msg, &tree, &parent_refs)
        .expect("commit")
}

fn set_branch(repo: &Repository, name: &str, oid: Oid) {
    let c = repo.find_commit(oid).expect("commit");
    repo.branch(name, &c, true).expect("branch");
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

/// Structural invariants that must hold for any layout.
fn assert_graph_invariants(g: &super::graph::CommitGraph) {
    assert!(
        g.rows.first().is_some_and(|r| r.edges.is_empty()),
        "first row has no incoming edges"
    );
    for (i, row) in g.rows.iter().enumerate() {
        assert!(row.lane < g.max_lanes, "row {i}: lane within max_lanes");
        for e in &row.edges {
            assert!(
                e.0 < g.max_lanes && e.1 < g.max_lanes,
                "row {i}: edge {e:?} within bounds"
            );
        }
    }
    // Every parent relation is reachable: each non-root commit's parents
    // appear later in the list.
    let index: std::collections::HashMap<&str, usize> = g
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.as_str(), i))
        .collect();
    for (i, row) in g.rows.iter().enumerate() {
        for p in &row.parents {
            let pi = index.get(p.as_str()).copied().expect("parent present");
            assert!(pi > i, "topological order");
            // The parent row must receive an edge landing on its lane.
            let prow = &g.rows[pi];
            assert!(
                prow.edges.iter().any(|e| e.1 == prow.lane),
                "parent {} has an incoming edge into its lane",
                prow.short_id
            );
        }
    }
}

#[test]
fn graph_linear_history_uses_one_lane() {
    let (_d, repo) = init();
    let a = commit_file(&repo, "f", "1", "a", &[], 1);
    let b = commit_file(&repo, "f", "2", "b", &[a], 2);
    let c = commit_file(&repo, "f", "3", "c", &[b], 3);
    set_branch(&repo, "main", c);

    let g = build_graph(&repo, GraphOptions::default()).expect("graph");
    assert_eq!(g.rows.len(), 3);
    assert_eq!(g.max_lanes, 1);
    assert!(g.rows.iter().all(|r| r.lane == 0));
    assert_eq!(g.rows[0].refs[0].name, "main");
    assert_graph_invariants(&g);
}

#[test]
fn graph_branch_and_merge() {
    let (_d, repo) = init();
    let a = commit_file(&repo, "f", "a", "a", &[], 1);
    let b = commit_file(&repo, "f", "b", "b", &[a], 2);
    let c = commit_file(&repo, "g", "c", "c", &[a], 3);
    let m = commit_file(&repo, "h", "m", "merge", &[b, c], 4);
    let d = commit_file(&repo, "g", "d", "d (unmerged feature)", &[c], 5);
    set_branch(&repo, "main", m);
    set_branch(&repo, "feature", d);
    repo.tag_lightweight("v1", repo.find_commit(a).expect("a").as_object(), false)
        .expect("tag");

    let g = build_graph(&repo, GraphOptions::default()).expect("graph");
    assert_eq!(g.rows.len(), 5);
    assert!(g.max_lanes >= 2);
    assert_graph_invariants(&g);

    let row = |oid: Oid| {
        g.rows
            .iter()
            .find(|r| r.id == oid.to_string())
            .expect("row")
    };
    // The merge has two parents in two different lanes.
    assert_eq!(row(m).parents.len(), 2);
    // Root commit carries the tag badge.
    assert!(row(a).refs.iter().any(|r| r.name == "v1"));
    // Both branch lines converge on the root: >= 2 edges land on its lane.
    let ra = row(a);
    assert!(ra.edges.iter().filter(|e| e.1 == ra.lane).count() >= 2);
}

#[test]
fn graph_handles_empty_repository() {
    let (_d, repo) = init();
    let g = build_graph(&repo, GraphOptions::default()).expect("graph");
    assert!(g.rows.is_empty());
}

#[test]
fn graph_page_clamps() {
    let (_d, repo) = init();
    let mut prev = commit_file(&repo, "f", "0", "0", &[], 1);
    for i in 1..50 {
        prev = commit_file(&repo, "f", &i.to_string(), &i.to_string(), &[prev], i + 1);
    }
    set_branch(&repo, "main", prev);
    let g = build_graph(&repo, GraphOptions::default()).expect("graph");
    let page = g.page(45, 100);
    assert_eq!(page.total, 50);
    assert_eq!(page.rows.len(), 5);
    assert_eq!(g.page(500, 10).rows.len(), 0);
}

// ---------------------------------------------------------------------------
// Diff + staging
// ---------------------------------------------------------------------------

const BASE: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";

fn setup_modified() -> (tempfile::TempDir, Repository) {
    let (d, repo) = init();
    let a = commit_file(&repo, "file.txt", BASE, "base", &[], 1);
    set_branch(&repo, "main", a);
    repo.set_head("refs/heads/main").expect("head");
    // Two separate changes → two hunks with 1 line of context.
    fs::write(
        d.path().join("file.txt"),
        "l1\nL2 changed\nl3\nl4\nl5\nl6\nl7\nL8 changed\nl9\nl10\n",
    )
    .expect("modify");
    (d, repo)
}

fn index_content(repo: &Repository, path: &str) -> String {
    let index = repo.index().expect("index");
    let entry = index.get_path(Path::new(path), 0).expect("entry");
    let blob = repo.find_blob(entry.id).expect("blob");
    String::from_utf8(blob.content().to_vec()).expect("utf8")
}

fn request(action: StageAction, hunks: Vec<HunkSelection>) -> StageRequest {
    StageRequest {
        repo_id: Uuid::nil(),
        path: "file.txt".into(),
        action,
        context_lines: 1,
        hunks,
    }
}

fn headers(repo: &Repository, target: DiffTarget) -> Vec<String> {
    let params = diff::DiffParams {
        context_lines: 1,
        ignore_whitespace: false,
    };
    let d = diff::compute(repo, &target, params, None).expect("diff");
    let r = diff::to_result(&d, target, params).expect("result");
    r.files
        .first()
        .map(|f| f.hunks.iter().map(|h| h.header.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn diff_produces_structured_hunks() {
    let (_d, repo) = setup_modified();
    let params = diff::DiffParams {
        context_lines: 1,
        ignore_whitespace: false,
    };
    let d = diff::compute(&repo, &DiffTarget::WorkingTree, params, None).expect("diff");
    let r = diff::to_result(&d, DiffTarget::WorkingTree, params).expect("result");
    assert_eq!(r.files.len(), 1);
    let f = &r.files[0];
    assert_eq!((f.additions, f.deletions), (2, 2));
    assert_eq!(f.hunks.len(), 2);
    let kinds: Vec<_> = f.hunks[0].lines.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        vec![
            DiffLineKind::Context,
            DiffLineKind::Deletion,
            DiffLineKind::Addition,
            DiffLineKind::Context
        ]
    );
    assert_eq!(f.hunks[0].lines[2].content, "L2 changed");
    assert!(!f.hunks[0].lines[2].content.ends_with('\n'));
}

#[test]
fn stage_single_hunk() {
    let (_d, repo) = setup_modified();
    let h = headers(&repo, DiffTarget::WorkingTree);
    stage::apply(
        &repo,
        &request(
            StageAction::Stage,
            vec![HunkSelection {
                hunk_index: 1,
                header: h[1].clone(),
                lines: None,
            }],
        ),
    )
    .expect("stage");
    assert_eq!(
        index_content(&repo, "file.txt"),
        "l1\nl2\nl3\nl4\nl5\nl6\nl7\nL8 changed\nl9\nl10\n"
    );
    // The first hunk is still unstaged.
    assert_eq!(headers(&repo, DiffTarget::WorkingTree).len(), 1);
}

#[test]
fn stage_only_the_addition_line() {
    let (_d, repo) = setup_modified();
    let h = headers(&repo, DiffTarget::WorkingTree);
    // Hunk 0 lines: [ctx l1, -l2, +L2 changed, ctx l3]; select only the "+".
    stage::apply(
        &repo,
        &request(
            StageAction::Stage,
            vec![HunkSelection {
                hunk_index: 0,
                header: h[0].clone(),
                lines: Some(vec![2]),
            }],
        ),
    )
    .expect("stage");
    assert_eq!(
        index_content(&repo, "file.txt"),
        format!("l1\nl2\nL2 changed\n{}", &BASE[6..])
    );
}

#[test]
fn unstage_hunk_restores_head_content() {
    let (_d, repo) = setup_modified();
    stage::apply(&repo, &request(StageAction::Stage, vec![])).expect("stage whole file");
    let h = headers(&repo, DiffTarget::Index);
    assert_eq!(h.len(), 2);
    stage::apply(
        &repo,
        &request(
            StageAction::Unstage,
            vec![HunkSelection {
                hunk_index: 0,
                header: h[0].clone(),
                lines: None,
            }],
        ),
    )
    .expect("unstage");
    assert_eq!(
        index_content(&repo, "file.txt"),
        "l1\nl2\nl3\nl4\nl5\nl6\nl7\nL8 changed\nl9\nl10\n"
    );
}

#[test]
fn stale_header_is_rejected() {
    let (_d, repo) = setup_modified();
    let err = stage::apply(
        &repo,
        &request(
            StageAction::Stage,
            vec![HunkSelection {
                hunk_index: 0,
                header: "@@ -1,1 +1,1 @@".into(),
                lines: None,
            }],
        ),
    )
    .expect_err("must be stale");
    assert_eq!(err.kind(), "stale");
}

#[test]
fn stage_handles_missing_trailing_newline() {
    let (d, repo) = init();
    let a = commit_file(&repo, "file.txt", "a\nb", "base", &[], 1);
    set_branch(&repo, "main", a);
    repo.set_head("refs/heads/main").expect("head");
    fs::write(d.path().join("file.txt"), "a\nc").expect("modify");

    let params = diff::DiffParams {
        context_lines: 1,
        ignore_whitespace: false,
    };
    let dd = diff::compute(&repo, &DiffTarget::WorkingTree, params, None).expect("diff");
    let r = diff::to_result(&dd, DiffTarget::WorkingTree, params).expect("result");
    let hunk = &r.files[0].hunks[0];
    let del = hunk
        .lines
        .iter()
        .position(|l| l.kind == DiffLineKind::Deletion)
        .expect("del");
    let add = hunk
        .lines
        .iter()
        .position(|l| l.kind == DiffLineKind::Addition)
        .expect("add");
    assert!(hunk.lines[del].no_newline && hunk.lines[add].no_newline);

    // Stage only the addition: both lines must survive, not be glued together.
    stage::apply(
        &repo,
        &request(
            StageAction::Stage,
            vec![HunkSelection {
                hunk_index: 0,
                header: hunk.header.clone(),
                lines: Some(vec![add]),
            }],
        ),
    )
    .expect("stage");
    assert_eq!(index_content(&repo, "file.txt"), "a\nb\nc");
}

#[test]
fn path_traversal_is_rejected() {
    let (_d, repo) = setup_modified();
    let mut req = request(StageAction::Stage, vec![]);
    req.path = "../outside.txt".into();
    assert_eq!(
        stage::apply(&repo, &req).expect_err("traversal").kind(),
        "invalidInput"
    );
}

/// Layout benchmark on a real repository:
/// `OCTONODE_BENCH_REPO=/path/to/repo cargo test --release bench_graph -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_graph() {
    let Some(path) = std::env::var_os("OCTONODE_BENCH_REPO") else {
        return;
    };
    let repo = Repository::open(path).expect("open bench repo");
    let started = std::time::Instant::now();
    let g = build_graph(&repo, GraphOptions::default()).expect("graph");
    let elapsed = started.elapsed();
    let payload = serde_json::to_vec(&g.page(0, 500)).expect("json").len();
    println!(
        "{} commits, {} lanes, layout {:?}, 500-row page = {} KiB",
        g.rows.len(),
        g.max_lanes,
        elapsed,
        payload / 1024
    );
    assert_graph_invariants(&g);
}
