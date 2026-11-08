//! Native file dialogs. Tests never open them (they would block).

use std::path::{Path, PathBuf};

pub fn save(name: &str, filter: &str, exts: &[&str]) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    rfd::FileDialog::new()
        .set_file_name(name)
        .add_filter(filter, exts)
        .save_file()
}

pub fn open(filters: &[(&str, &[&str])]) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let mut d = rfd::FileDialog::new();
    for (name, exts) in filters {
        d = d.add_filter(*name, exts);
    }
    d.pick_file()
}

pub fn folder(start: &Path) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    rfd::FileDialog::new().set_directory(start).pick_folder()
}
