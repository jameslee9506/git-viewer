//! Native macOS menu. Custom items emit a `menu` event ({ id }) to the frontend.

use crate::model;
use tauri::menu::{
    AboutMetadata, Menu, MenuBuilder, MenuEvent, MenuItem, MenuItemBuilder, SubmenuBuilder, HELP_SUBMENU_ID,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Emitter, Wry};

/// A custom menu item; `id` reaches the frontend as `MenuEvent.id` (`MenuId` in types.ts).
struct Item {
    id: &'static str,
    text: &'static str,
    accelerator: &'static str,
}

const SETTINGS: Item = Item { id: "settings", text: "Settings…", accelerator: "CmdOrCtrl+," };
const OPEN: Item = Item { id: "open", text: "Open…", accelerator: "CmdOrCtrl+O" };
const NEW_TAB: Item = Item { id: "new-tab", text: "New Tab", accelerator: "CmdOrCtrl+T" };
const NEW_GROUP: Item = Item { id: "new-group", text: "New Group", accelerator: "CmdOrCtrl+Shift+N" };
const CLOSE_TAB: Item = Item { id: "close-tab", text: "Close Tab", accelerator: "CmdOrCtrl+W" };
const REFRESH: Item = Item { id: "refresh", text: "Refresh", accelerator: "CmdOrCtrl+R" };
const FIND: Item = Item { id: "find", text: "Find", accelerator: "CmdOrCtrl+F" };
const TOGGLE_DETAILS: Item = Item { id: "toggle-details", text: "Toggle Details", accelerator: "CmdOrCtrl+K" };
const SIMPLE_VIEW: Item = Item { id: "simple-view", text: "Simple View", accelerator: "CmdOrCtrl+Shift+L" };
const ZOOM_IN: Item = Item { id: "zoom-in", text: "Zoom In", accelerator: "CmdOrCtrl+=" };
const ZOOM_OUT: Item = Item { id: "zoom-out", text: "Zoom Out", accelerator: "CmdOrCtrl+-" };
const ZOOM_RESET: Item = Item { id: "zoom-reset", text: "Actual Size", accelerator: "CmdOrCtrl+0" };

/// Every custom item: the ids `handle` forwards.
const CUSTOM: [&Item; 12] = [
    &SETTINGS,
    &OPEN,
    &NEW_TAB,
    &NEW_GROUP,
    &CLOSE_TAB,
    &REFRESH,
    &FIND,
    &TOGGLE_DETAILS,
    &SIMPLE_VIEW,
    &ZOOM_IN,
    &ZOOM_OUT,
    &ZOOM_RESET,
];

fn custom(app: &AppHandle, item: &Item) -> tauri::Result<MenuItem<Wry>> {
    MenuItemBuilder::with_id(item.id, item.text).accelerator(item.accelerator).build(app)
}

/// App / File / Edit / View / Window / Help menu bar. No predefined Close Window item: ⌘W closes
/// the tab (custom `close-tab`).
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let pkg = app.package_info();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: app.config().bundle.copyright.clone(),
        ..Default::default()
    };
    let app_menu = SubmenuBuilder::new(app, &pkg.name)
        .about(Some(about))
        .separator()
        .item(&custom(app, &SETTINGS)?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&custom(app, &OPEN)?)
        .item(&custom(app, &NEW_TAB)?)
        .item(&custom(app, &NEW_GROUP)?)
        .separator()
        .item(&custom(app, &CLOSE_TAB)?)
        .build()?;
    // Predefined Edit items are required: without them copy/paste shortcuts do nothing in the webview.
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&custom(app, &REFRESH)?)
        .item(&custom(app, &FIND)?)
        .separator()
        .item(&custom(app, &TOGGLE_DETAILS)?)
        .item(&custom(app, &SIMPLE_VIEW)?)
        .separator()
        .item(&custom(app, &ZOOM_IN)?)
        .item(&custom(app, &ZOOM_OUT)?)
        .item(&custom(app, &ZOOM_RESET)?)
        .separator()
        .fullscreen()
        .build()?;
    let window = SubmenuBuilder::with_id(app, WINDOW_SUBMENU_ID, "Window")
        .minimize()
        .maximize()
        .separator()
        .bring_all_to_front()
        .build()?;
    let help = SubmenuBuilder::with_id(app, HELP_SUBMENU_ID, "Help").build()?;
    MenuBuilder::new(app).items(&[&app_menu, &file, &edit, &view, &window, &help]).build()
}

/// Forward custom menu item clicks to the frontend as `menu` events.
pub fn handle(app: &AppHandle, event: MenuEvent) {
    let id: &str = event.id().as_ref();
    if CUSTOM.iter().any(|item| item.id == id) {
        let _ = app.emit("menu", model::MenuEvent { id: id.to_string() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_items_are_distinct() {
        for (i, a) in CUSTOM.iter().enumerate() {
            for b in &CUSTOM[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.accelerator, b.accelerator);
            }
        }
        let mut ids: Vec<&str> = CUSTOM.iter().map(|item| item.id).collect();
        ids.sort_unstable();
        let expected = [
            "close-tab",
            "find",
            "new-group",
            "new-tab",
            "open",
            "refresh",
            "settings",
            "simple-view",
            "toggle-details",
            "zoom-in",
            "zoom-out",
            "zoom-reset",
        ];
        assert_eq!(ids, expected);
    }
}
