//! Process-wide state: open repositories keyed by RepoInfo.path.

use crate::error::{AppError, AppResult};
use crate::git::graph_cache::{GraphCache, SearchKey};
use crate::model::{GraphSummary, RefsSnapshot, RepoInfo};
use crate::watcher::RepoWatcher;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct RepoHandle {
    pub info: RepoInfo,
    /// Current graph. Replaced only by a load with a newer ticket (= generation) that serves something
    /// else (see `install_graph`).
    pub graph: Mutex<Option<Arc<GraphCache>>>,
    /// Monotonic load ticket; each graph_load takes the next value as its generation (`begin_graph_load`).
    pub ticket: AtomicU64,
    pub watcher: Mutex<Option<RepoWatcher>>,
    /// Number of open_repo calls not yet balanced by close_repo.
    pub refcount: AtomicUsize,
    /// Last refs snapshot served by `repo_refs` (graph_load reuses it while fresh).
    pub refs_cache: Mutex<Option<(Instant, Arc<RefsSnapshot>)>>,
    /// Key and cancel flag of the last git-backed search (see `begin_search`).
    pub search_cancel: Mutex<Option<(SearchKey, Arc<AtomicBool>)>>,
    /// Cancel flag of the running blame. A repo tab shows one blame at a time, so a new blame (or
    /// closing the repo) cancels the previous one instead of letting it hold a process slot.
    pub blame_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Cancel flag of the running graph walk; a newer graph_load cancels it.
    pub graph_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// Set once the repo's last tab closed: a fetch / pull / push still running is killed.
    pub stop: Arc<AtomicBool>,
    /// Cancel flag of the running background fetch (see `begin_background_fetch`).
    pub background_fetch: Mutex<Option<Arc<AtomicBool>>>,
}

impl RepoHandle {
    pub fn new(info: RepoInfo) -> Self {
        RepoHandle {
            info,
            graph: Mutex::new(None),
            ticket: AtomicU64::new(0),
            watcher: Mutex::new(None),
            refcount: AtomicUsize::new(1),
            refs_cache: Mutex::new(None),
            search_cancel: Mutex::new(None),
            blame_cancel: Mutex::new(None),
            graph_cancel: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            background_fetch: Mutex::new(None),
        }
    }

    /// Take the next load ticket (= the generation of the graph the load builds) and the cancel flag of
    /// its walk, cancelling the running walk (if any): rapid filter toggles must not run several full
    /// `git log` walks at once (the frontend ignores superseded loads). Both under one lock, so a walk is
    /// only ever cancelled by a load with a later ticket, never the newest load by an older one.
    pub fn begin_graph_load(&self) -> (u64, Arc<AtomicBool>) {
        let mut slot = self.graph_cancel.lock().unwrap();
        let ticket = self.ticket.fetch_add(1, Ordering::SeqCst) + 1;
        let flag = Arc::new(AtomicBool::new(false));
        if let Some(prev) = slot.replace(flag.clone()) {
            prev.store(true, Ordering::SeqCst);
        }
        (ticket, flag)
    }

    /// Cancel every git run of this repo once its last tab closed (nothing will show their results): the
    /// graph walk, the git-backed search, the blame and a network operation. Each would otherwise run to
    /// its end, holding a process slot, and the walk and the search this handle with its watcher.
    pub fn cancel_all(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.cancel_blame();
        let walk = self.graph_cancel.lock().unwrap().take();
        let search = self.search_cancel.lock().unwrap().take().map(|(_, flag)| flag);
        for flag in [walk, search].into_iter().flatten() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub fn store_refs(&self, snap: Arc<RefsSnapshot>) {
        *self.refs_cache.lock().unwrap() = Some((Instant::now(), snap));
    }

    /// The cached refs snapshot if it is younger than `max_age`.
    pub fn fresh_refs(&self, max_age: Duration) -> Option<Arc<RefsSnapshot>> {
        let slot = self.refs_cache.lock().unwrap();
        slot.as_ref().filter(|(at, _)| at.elapsed() <= max_age).map(|(_, s)| s.clone())
    }

    /// Register a git-backed search and return its cancel flag. A search with the key of the running
    /// one (a graph reload re-running the active search over the same walk) shares its flag and joins
    /// its git run; any other search cancels it.
    pub fn begin_search(&self, key: SearchKey) -> Arc<AtomicBool> {
        let mut slot = self.search_cancel.lock().unwrap();
        if let Some((k, flag)) = slot.as_ref() {
            if *k == key && !flag.load(Ordering::SeqCst) {
                return flag.clone();
            }
        }
        let flag = Arc::new(AtomicBool::new(false));
        if let Some((_, prev)) = slot.replace((key, flag.clone())) {
            prev.store(true, Ordering::SeqCst);
        }
        flag
    }

    /// Cancel the running blame and register a flag for a new one.
    pub fn begin_blame(&self) -> Arc<AtomicBool> {
        supersede(&self.blame_cancel)
    }

    /// Cancel the running blame, if any (nothing will show it).
    pub fn cancel_blame(&self) {
        if let Some(flag) = self.blame_cancel.lock().unwrap().take() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Register a background fetch and return its cancel flag. One still running is cancelled: two
    /// fetches of one repository race for the locks of the refs they both update.
    pub fn begin_background_fetch(&self) -> Arc<AtomicBool> {
        supersede(&self.background_fetch)
    }

    /// Cancel the running background fetch, if any: a fetch / pull / push the user asked for goes
    /// first. (Closing the repo's last tab stops it through `stop`.)
    pub fn cancel_background_fetch(&self) {
        if let Some(flag) = self.background_fetch.lock().unwrap().take() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// The current graph if its generation matches, else `AppError::Stale`.
    pub fn graph_at(&self, generation: u64) -> AppResult<Arc<GraphCache>> {
        let slot = self.graph.lock().unwrap();
        match slot.as_ref() {
            Some(c) if c.generation == generation => Ok(c.clone()),
            _ => Err(AppError::Stale),
        }
    }

    pub fn current_graph(&self) -> Option<Arc<GraphCache>> {
        self.graph.lock().unwrap().clone()
    }

    /// Install `cache` unless a newer generation is already installed, and return the summary to
    /// serve. When the installed graph serves exactly the same rows, it stays and its summary (with
    /// its generation) is returned: the frontend keeps its rows and search results, so a reload that
    /// changes nothing (a window focus refresh, usually) costs it nothing.
    pub fn install_graph(&self, cache: GraphCache) -> GraphSummary {
        let mut slot = self.graph.lock().unwrap();
        match slot.as_ref() {
            Some(cur) if cur.same_graph(&cache) => cur.summary(),
            // A load with a later ticket installed its graph first: this one is not kept (the frontend
            // sees the newer generation and loads again).
            Some(cur) if cur.generation > cache.generation => cache.summary(),
            _ => {
                let summary = cache.summary();
                *slot = Some(Arc::new(cache));
                summary
            }
        }
    }

}

/// Put a fresh cancel flag in `slot` and set the one it replaces: the operation that registered it
/// is superseded.
fn supersede(slot: &Mutex<Option<Arc<AtomicBool>>>) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Some(prev) = slot.lock().unwrap().replace(flag.clone()) {
        prev.store(true, Ordering::SeqCst);
    }
    flag
}

pub struct AppState {
    pub repos: Mutex<HashMap<String, Arc<RepoHandle>>>,
    /// Absolute paths to open at launch: CLI arguments plus folders opened (Dock / `open -a`) before
    /// the frontend asked for them.
    pub startup_paths: Mutex<Vec<String>>,
    /// Set once the frontend fetched `startup_paths`; later opens are delivered as events only.
    pub startup_consumed: AtomicBool,
}

impl AppState {
    pub fn new(startup_paths: Vec<String>) -> Self {
        AppState {
            repos: Mutex::new(HashMap::new()),
            startup_paths: Mutex::new(startup_paths),
            startup_consumed: AtomicBool::new(false),
        }
    }

    /// Queue paths opened before the frontend is ready. Returns false once startup paths were consumed.
    pub fn queue_startup_paths(&self, paths: &[String]) -> bool {
        // Check under the lock `startup_paths` takes to consume the queue, so a path can't land in a
        // queue nobody reads again.
        let mut queue = self.startup_paths.lock().unwrap();
        if self.startup_consumed.load(Ordering::SeqCst) {
            return false;
        }
        queue.extend(paths.iter().cloned());
        true
    }

    pub fn get(&self, repo: &str) -> AppResult<Arc<RepoHandle>> {
        self.repos
            .lock()
            .unwrap()
            .get(repo)
            .cloned()
            .ok_or_else(|| AppError::msg(format!("repository not open: {repo}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SearchMode;

    fn key(walk: u64, text: &str) -> SearchKey {
        SearchKey { walk, mode: SearchMode::Content, text: text.into() }
    }

    fn handle() -> RepoHandle {
        RepoHandle::new(RepoInfo {
            path: "/r".into(),
            name: "r".into(),
            git_dir: "/r/.git".into(),
            common_dir: "/r/.git".into(),
            is_bare: false,
            is_linked_worktree: false,
        })
    }

    #[test]
    fn a_new_blame_cancels_the_running_one() {
        let h = handle();
        let first = h.begin_blame();
        assert!(!first.load(Ordering::SeqCst));
        let second = h.begin_blame();
        assert!(first.load(Ordering::SeqCst));
        assert!(!second.load(Ordering::SeqCst));
        // Blames and searches are cancelled independently.
        let search = h.begin_search(key(1, "x"));
        assert!(!second.load(Ordering::SeqCst));
        h.cancel_blame();
        assert!(second.load(Ordering::SeqCst));
        assert!(!search.load(Ordering::SeqCst));
        // Nothing left to cancel; the next blame starts clean.
        h.cancel_blame();
        assert!(!h.begin_blame().load(Ordering::SeqCst));
    }

    #[test]
    fn a_graph_load_cancels_only_walks_of_earlier_tickets() {
        let h = handle();
        let (t1, first) = h.begin_graph_load();
        let (t2, second) = h.begin_graph_load();
        assert!(t2 > t1);
        assert!(first.load(Ordering::SeqCst) && !second.load(Ordering::SeqCst));
        // A closed repo stops the walk, the git search, the blame and a network operation alike.
        let search = h.begin_search(key(1, "x"));
        let blame = h.begin_blame();
        assert!(!h.stop.load(Ordering::SeqCst));
        h.cancel_all();
        assert!(second.load(Ordering::SeqCst) && search.load(Ordering::SeqCst) && blame.load(Ordering::SeqCst));
        assert!(h.stop.load(Ordering::SeqCst));
        let (t3, third) = h.begin_graph_load();
        assert!(t3 > t2 && !third.load(Ordering::SeqCst));
    }

    #[test]
    fn a_background_fetch_is_cancelled_on_request_and_by_the_next_one() {
        let h = handle();
        // Nothing runs: nothing to cancel, and the next fetch starts clean.
        h.cancel_background_fetch();
        let first = h.begin_background_fetch();
        assert!(!first.load(Ordering::SeqCst));
        h.cancel_background_fetch();
        assert!(first.load(Ordering::SeqCst));
        let second = h.begin_background_fetch();
        let third = h.begin_background_fetch();
        assert!(second.load(Ordering::SeqCst) && !third.load(Ordering::SeqCst));
        // Independent of the flag a closed repo sets (the fetch watches both).
        assert!(!h.stop.load(Ordering::SeqCst));
    }

    #[test]
    fn only_a_different_search_cancels_the_running_one() {
        let h = handle();
        let first = h.begin_search(key(1, "x"));
        // A reload re-running the same query over the same walk joins it.
        assert!(Arc::ptr_eq(&first, &h.begin_search(key(1, "x"))));
        assert!(!first.load(Ordering::SeqCst));
        // Another query, or the same one over another walk, cancels it.
        let other = h.begin_search(key(1, "y"));
        assert!(first.load(Ordering::SeqCst));
        let moved = h.begin_search(key(2, "y"));
        assert!(other.load(Ordering::SeqCst));
        assert!(!moved.load(Ordering::SeqCst));
        // A query that comes back after being cancelled gets a new flag.
        let back = h.begin_search(key(1, "x"));
        assert!(moved.load(Ordering::SeqCst));
        assert!(!back.load(Ordering::SeqCst) && !Arc::ptr_eq(&back, &first));
    }
}
