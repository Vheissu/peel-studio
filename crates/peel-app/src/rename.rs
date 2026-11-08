//! A small floating text field for renaming things in place.

use peel_ui::{Id as W, Rect, Ui};

#[derive(Default)]
struct RenameState {
    text: String,
    anchor: Rect,
}

/// Opens the rename field for `id` below `anchor`, prefilled and selected.
pub fn open(ui: &mut Ui, id: W, anchor: Rect, current: &str) {
    let st = ui.state::<RenameState>(id.with("rename"));
    st.text = current.to_string();
    st.anchor = anchor;
    ui.open_popup(id, anchor);
    ui.focus_text_edit(id.with("field"), true);
}

/// Draws the field while open. Returns the new name when Enter commits a
/// non-empty value; Escape or a click elsewhere cancels.
pub fn show(ui: &mut Ui, id: W) -> Option<String> {
    if !ui.is_popup_open(id) {
        return None;
    }
    let (mut text, anchor) = {
        let st = ui.state::<RenameState>(id.with("rename"));
        (st.text.clone(), st.anchor)
    };
    let screen = ui.screen();
    let w = 240.0;
    let r = Rect {
        x: anchor.x.min(screen.right() - w - 8.0),
        y: anchor.bottom() + 4.0,
        w,
        h: 44.0,
    };
    let mut committed = None;
    ui.popup_panel(id, r, |ui, r| {
        let resp = ui.text_edit(id.with("field"), r.shrink(7.0), &mut text, "Name");
        if resp.changed {
            committed = Some(text.trim().to_string());
        }
    });
    ui.state::<RenameState>(id.with("rename")).text = text;
    if committed.is_some() || !ui.has_focus(id.with("field")) {
        ui.close_popup();
    }
    committed.filter(|t| !t.is_empty())
}
