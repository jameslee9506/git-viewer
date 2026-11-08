//! Tauri commands. Thin wrappers: resolve state, run the blocking git work off the main thread.
//! Every command is `async` (sync commands would run on the main thread and freeze the UI).

use crate::error::{AppError, AppResult};
use crate::git;
use crate::git::graph_cache::GraphCache;
use crate::model::*;
use crate::state::{AppState, RepoHandle};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

async fn blocking<T, F>(f: F) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::msg(format!("background task failed: {e}")))?
}

#[tauri::command]
pub async fn startup_paths(state: State<'_, AppState>) -> AppResult<Vec<String>> {
    let mut paths = state.startup_paths.lock().unwrap();
    state.startup_consumed.store(true, Ordering::SeqCst);
    Ok(std::mem::take(&mut *paths))
}

const USER_CSS_TEMPLATE: &str = "/* Git Viewer user stylesheet. Loaded after the built-in theme.\n   Override any token from src/styles/tokens.css, e.g.\n   :root { --accent: #8ab4f8; }\n   Keep surface, row-state and lane colors opaque.\n*/\n";

#[tauri::command]
pub async fn user_css(app: AppHandle, create: bool) -> AppResult<UserCss> {
    let dir = app.path().app_config_dir().map_err(|e| AppError::msg(e.to_string()))?;
    let path = dir.join("user.css");
    blocking(move || {
        if create && !path.exists() {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, USER_CSS_TEMPLATE)?;
        }
        let css = std::fs::read_to_string(&path).ok();
        Ok(UserCss { path: path.to_string_lossy().into_owned(), css })
    })
    .await
}

#[tauri::command]
pub async fn open_repo(app: AppHandle, state: State<'_, AppState>, path: String) -> AppResult<RepoInfo> {
    let info = blocking(move || git::repo::open(Path::new(&path))).await?;
    if let Some(h) = state.repos.lock().unwrap().get(&info.path) {
        h.refcount.fetch_add(1, Ordering::SeqCst);
        return Ok(info);
    }
    let handle = Arc::new(RepoHandle::new(info.clone()));
    let watch_info = info.clone();
    let watcher = blocking(move || crate::watcher::start(app, &watch_info)).await;
    match watcher {
        Ok(w) => *handle.watcher.lock().unwrap() = Some(w),
        Err(e) => eprintln!("[git-viewer] watcher unavailable for {}: {e}", info.path),
    }
    let mut repos = state.repos.lock().unwrap();
    match repos.get(&info.path) {
        // Lost a race with a concurrent open of the same repo: keep the existing handle.
        Some(existing) => {
            existing.refcount.fetch_add(1, Ordering::SeqCst);
        }
        None => {
            repos.insert(info.path.clone(), handle);
        }
    }
    Ok(info)
}

#[tauri::command]
pub async fn close_repo(state: State<'_, AppState>, repo: String) -> AppResult<()> {
    let removed = {
        let mut repos = state.repos.lock().unwrap();
        match repos.get(&repo) {
            Some(h) if h.refcount.fetch_sub(1, Ordering::SeqCst) <= 1 => repos.remove(&repo),
            _ => None,
        }
    };
    // Dropping the last handle stops the watcher (joins its thread): do it outside the lock, off the
    // async workers.
    if let Some(h) = removed {
        // Its tab is gone: a graph walk, git search or blame still running would only hold a process slot
        // (and, for the walk and the search, this handle with its watcher) until it ends.
        h.cancel_all();
        git::history::cancel(Path::new(&repo));
        let _ = tauri::async_runtime::spawn_blocking(move || drop(h)).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn repo_refs(state: State<'_, AppState>, repo: String) -> AppResult<RefsSnapshot> {
    let h = state.get(&repo)?;
    blocking(move || {
        let snap = git::refs::snapshot(&h.info)?;
        h.store_refs(Arc::new(snap.clone()));
        Ok(snap)
    })
    .await
}

/// Ahead/behind of the local branches against their upstreams. Not part of `repo_refs`, which every
/// refresh and graph load waits for: the walks behind the counts can take seconds on a big history.
/// The frontend asks once the graph is in. `known` is its previous answer: branches whose (tip,
/// upstream tip) pair it holds are not walked again.
#[tauri::command]
pub async fn repo_track(
    state: State<'_, AppState>,
    repo: String,
    known: Vec<BranchTrack>,
) -> AppResult<Vec<BranchTrack>> {
    // Not the handle: a walk still running when the tab closes must not keep its watcher alive.
    let path = state.get(&repo)?.info.path.clone();
    blocking(move || git::refs::track(Path::new(&path), &known)).await
}

/// How long a `repo_refs` snapshot may be reused by `graph_load` (the frontend loads refs, then the graph).
const REFS_REUSE: std::time::Duration = std::time::Duration::from_millis(1500);

#[tauri::command]
pub async fn worktree_status(cwd: String) -> AppResult<WorkingStatus> {
    blocking(move || git::status::status(Path::new(&cwd))).await
}

#[tauri::command]
pub async fn graph_load(
    state: State<'_, AppState>,
    repo: String,
    opts: GraphOptions,
    wip: bool,
) -> AppResult<GraphSummary> {
    let h = state.get(&repo)?;
    // The ticket is taken before the blocking pool picks the work up, so loads get their generations in
    // the order they were asked for, as far as the async runtime keeps it (the newest wins the install).
    // It cancels the walks of earlier tickets, which then fail with "cancelled".
    let (ticket, cancel) = h.begin_graph_load();
    blocking(move || {
        let prev = h.current_graph();
        let refs = h.fresh_refs(REFS_REUSE);
        let cache = GraphCache::build(&h.info, &opts, wip, ticket, prev.as_deref(), refs, Some(cancel))?;
        // The generation served: the current one when the load changed nothing.
        Ok(h.install_graph(cache))
    })
    .await
}

#[tauri::command]
pub async fn graph_rows(
    state: State<'_, AppState>,
    repo: String,
    generation: u64,
    start: u32,
    end: u32,
) -> AppResult<Vec<GraphRow>> {
    let cache = state.get(&repo)?.graph_at(generation)?;
    Ok(cache.rows(start, end))
}

#[tauri::command]
pub async fn graph_find(
    state: State<'_, AppState>,
    repo: String,
    generation: u64,
    oid: String,
) -> AppResult<Option<u32>> {
    let cache = state.get(&repo)?.graph_at(generation)?;
    Ok(cache.find(&oid))
}

#[tauri::command]
pub async fn graph_search(
    state: State<'_, AppState>,
    repo: String,
    generation: u64,
    query: SearchQuery,
) -> AppResult<Vec<u32>> {
    let h = state.get(&repo)?;
    let cache = h.graph_at(generation)?;
    // A reload re-running the active search over the same walk joins its git run; any other git-backed
    // search kills it.
    let cancel = (query.mode != SearchMode::Quick).then(|| h.begin_search(cache.search_key(&query)));
    blocking(move || cache.search(&h.info, &query, cancel)).await
}

#[tauri::command]
pub async fn commit_details(repo: String, oid: String) -> AppResult<CommitDetails> {
    blocking(move || git::commit::details(Path::new(&repo), &oid)).await
}

#[tauri::command]
pub async fn diff_files(repo: String, target: DiffTarget) -> AppResult<Vec<FileChange>> {
    blocking(move || git::commit::diff_files(Path::new(&repo), &target)).await
}

#[tauri::command]
pub async fn file_diff(repo: String, req: DiffRequest) -> AppResult<FileDiff> {
    blocking(move || git::diff::file_diff(Path::new(&repo), &req)).await
}

#[tauri::command]
pub async fn read_blob(
    repo: String,
    spec: BlobSpec,
    encoding: BlobEncoding,
    max_bytes: Option<u64>,
) -> AppResult<BlobContent> {
    blocking(move || git::blob::read_blob(Path::new(&repo), &spec, encoding, max_bytes)).await
}

#[tauri::command]
pub async fn list_tree(repo: String, rev: String, path: String, recursive: bool) -> AppResult<Vec<TreeEntry>> {
    blocking(move || git::tree::list_tree(Path::new(&repo), &rev, &path, recursive)).await
}

/// A repo tab shows one blame at a time: a new blame kills the previous run (its result would be
/// dropped anyway), which then fails with "cancelled".
#[tauri::command]
pub async fn blame_file(
    state: State<'_, AppState>,
    repo: String,
    path: String,
    rev: Option<String>,
    cwd: Option<String>,
    ignore_whitespace: bool,
) -> AppResult<BlameData> {
    let cancel = state.get(&repo).ok().map(|h| h.begin_blame());
    blocking(move || {
        git::blame::blame(Path::new(&repo), &path, rev.as_deref(), cwd.as_deref(), ignore_whitespace, cancel)
    })
    .await
}

#[tauri::command]
pub async fn file_history(
    repo: String,
    path: String,
    rev: Option<String>,
    follow: bool,
    limit: u32,
) -> AppResult<Vec<HistoryEntry>> {
    blocking(move || git::history::file_history(Path::new(&repo), &path, rev.as_deref(), follow, limit)).await
}
