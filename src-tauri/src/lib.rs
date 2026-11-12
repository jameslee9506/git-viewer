mod commands;
pub mod error;
pub mod git;
mod menu;
pub mod model;
mod state;
mod watcher;

use tauri::{Emitter, Manager};

/// Absolute paths of non-flag CLI arguments (repos to open at launch).
fn cli_paths() -> Vec<String> {
    let cwd = std::env::current_dir().ok();
    std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .filter_map(|a| {
            let p = std::path::PathBuf::from(&a);
            let abs = if p.is_absolute() { p } else { cwd.as_ref()?.join(p) };
            abs.exists().then(|| abs.to_string_lossy().into_owned())
        })
        .collect()
}

pub fn run() {
    // The PATH every git runs with comes from the login shell (about 0.2 s): resolve it now, off the
    // main thread, so the first repo opens without that wait (a git started earlier waits for it).
    std::thread::spawn(|| {
        git::cmd::path_env();
    });
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state::AppState::new(cli_paths()))
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .invoke_handler(tauri::generate_handler![
            commands::startup_paths,
            commands::user_css,
            commands::open_repo,
            commands::close_repo,
            commands::repo_clone,
            commands::repo_clone_cancel,
            commands::repo_init,
            commands::repo_refs,
            commands::repo_track,
            commands::worktree_status,
            commands::graph_load,
            commands::graph_rows,
            commands::graph_find,
            commands::graph_search,
            commands::commit_details,
            commands::diff_files,
            commands::file_diff,
            commands::read_blob,
            commands::list_tree,
            commands::blame_file,
            commands::file_history,
            commands::wt_stage,
            commands::wt_stage_all,
            commands::wt_unstage,
            commands::wt_unstage_all,
            commands::wt_commit,
            commands::branch_create,
            commands::tag_create,
            commands::ref_exclusive,
            commands::branch_delete,
            commands::tag_delete,
            commands::head_checkout,
            commands::head_reset,
            commands::head_unreferenced,
            commands::commit_pick,
            commands::wt_pick_continue,
            commands::wt_pick_abort,
            commands::stash_push,
            commands::stash_pop,
            commands::stash_apply,
            commands::stash_drop,
            commands::remote_fetch,
            commands::remote_fetch_background,
            commands::remote_fetch_cancel,
            commands::remote_pull,
            commands::remote_push,
            commands::remote_delete_branch,
            commands::remote_delete_tag,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        // Folders dropped on the Dock icon / `open -a` arrive as Opened events on macOS.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = event {
            let paths: Vec<String> = urls
                .iter()
                .filter_map(|u| u.to_file_path().ok())
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            if !paths.is_empty() {
                // Before the frontend fetched startup paths (cold start) queue them; afterwards the
                // `open-paths` listener is live. Opening the same repo twice is deduplicated by the UI.
                handle.state::<state::AppState>().queue_startup_paths(&paths);
                let _ = handle.emit("open-paths", paths);
                // An open-documents event activates the app but, unlike a Dock click (Reopen), does
                // not deminiaturize its window, so the new tab would land in a window the user
                // can't see. Restore before focusing: tao's set_focus is a no-op while minimized.
                // `None` only on a cold start before the configured window exists.
                if let Some(w) = handle.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (handle, event);
    });
}
