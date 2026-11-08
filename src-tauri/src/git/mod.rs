//! Git access layer. Everything shells out to the git CLI through `cmd::Git` and parses its output.
//! Functions here are synchronous and may block; commands call them via `spawn_blocking`.

pub mod blame;
pub mod blob;
pub mod cmd;
pub mod commit;
pub mod diff;
pub mod graph;
pub mod graph_cache;
pub mod history;
pub mod log;
pub mod refs;
pub mod repo;
pub mod search;
pub mod status;
pub mod tree;
