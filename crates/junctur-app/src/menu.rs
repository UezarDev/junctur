//! The window's menu bar (2g): File > Open recording…; View > Side panel, Talking now overlay, and
//! the side panel's tabs (Transcript, Captions, Properties).
//!
//! Native rather than drawn by the page: an HTML strip would sit over mpv's surface and need a
//! margin of its own, and the menu works on the start window too. Every item is sent to the page as
//! a [`EVENT`] event with its id, and the page does the rest, as it does for its own buttons. The
//! View items carry check marks; the menu flips one when it is clicked, and the page sets it when
//! the same thing is toggled another way (the T key), through [`set_check`].

use tauri::menu::{
    CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// The event a menu click is sent as; its payload is the item's id.
pub const EVENT: &str = "menu";
pub const OPEN: &str = "open";
pub const PANEL: &str = "panel";
pub const TALKING: &str = "talking";
/// The side panel's tabs, shown (and the panel opened) by id.
pub const TABS: [(&str, &str); 3] = [
    ("transcript", "Transcript"),
    ("captions", "Captions"),
    ("properties", "Properties"),
];

/// The View items, kept to set their check marks.
struct Checks<R: Runtime> {
    panel: CheckMenuItem<R>,
    talking: CheckMenuItem<R>,
}

/// The menu for the main window. The overlay starts unchecked: it is off by default (2g's LV3),
/// and the page checks it once it has read the viewer's choice.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    let open = MenuItemBuilder::with_id(OPEN, "Open recording…")
        .accelerator("CmdOrCtrl+O")
        .build(app)?;
    // No accelerator for T: a native one would take the key from the transcript's inputs too;
    // the page's own key handler skips inputs.
    let panel = CheckMenuItemBuilder::with_id(PANEL, "Side panel (T)")
        .checked(true)
        .build(app)?;
    let talking = CheckMenuItemBuilder::with_id(TALKING, "Talking now overlay")
        .checked(false)
        .build(app)?;
    let file = SubmenuBuilder::new(app, "File").item(&open).build()?;
    let mut view = SubmenuBuilder::new(app, "View")
        .item(&panel)
        .item(&talking)
        .separator();
    for (id, label) in TABS {
        view = view.item(&MenuItemBuilder::with_id(id, label).build(app)?);
    }
    let view = view.build()?;
    let menu = MenuBuilder::new(app).item(&file).item(&view).build()?;
    app.manage(Checks { panel, talking });
    app.on_menu_event(|app, e| {
        let _ = app.emit(EVENT, e.id().0.clone());
    });
    Ok(menu)
}

/// Sets View item `id`'s check mark.
pub fn set_check<R: Runtime>(app: &AppHandle<R>, id: &str, checked: bool) -> Result<(), String> {
    let checks = app
        .try_state::<Checks<R>>()
        .ok_or("the window has no menu")?;
    let item = match id {
        PANEL => &checks.panel,
        TALKING => &checks.talking,
        _ => return Err(format!("no menu check {id:?}")),
    };
    item.set_checked(checked).map_err(|e| e.to_string())
}
