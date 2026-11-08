//! Loading `.clap` files: bundle resolution, `clap_entry` and the factory.
//!
//! A CLAP library must be initialised once and de-initialised once for each
//! time it is loaded. Several devices (and the scanner) can use the same
//! file at once, so loaded files are shared through a process-wide registry
//! keyed by canonical path. The file stays initialised while any
//! [`ModuleHandle`] to it exists; the last one to go runs `clap_entry.deinit`,
//! under the registry lock so a concurrent open can never overlap a deinit.
//!
//! The shared library itself is **never unloaded**. Plugins routinely leave
//! threads, timers and static destructors that would run into unmapped code
//! after `dlclose`, and a crash there is far worse than a few mapped
//! megabytes. The consequence is that a plugin updated on disk is not picked
//! up until the application restarts, and a later load of the same file
//! runs `init` again on the same image, which the CLAP entry contract allows.

use std::ffi::{CStr, CString};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::version::{CLAP_VERSION, clap_version, clap_version_is_compatible};

use crate::error::ClapError;
use crate::thread::RoleScope;

pub(crate) fn version_string(v: clap_version) -> String {
    format!("{}.{}.{}", v.major, v.minor, v.revision)
}

/// Finds the shared library inside a `.clap` path.
///
/// On Windows and Linux a `.clap` is the library itself. On macOS it is a
/// bundle directory and the binary lives in `Contents/MacOS/`: the name comes
/// from `CFBundleExecutable` in `Info.plist`, then the bundle's own name,
/// then the only file in the folder.
pub(crate) fn resolve_binary(path: &Path) -> Result<PathBuf, ClapError> {
    let meta = std::fs::metadata(path).map_err(|e| ClapError::Io {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    if !meta.is_dir() {
        return Ok(path.to_path_buf());
    }
    let macos = path.join("Contents").join("MacOS");
    if let Some(name) = bundle_executable(&path.join("Contents").join("Info.plist")) {
        let candidate = macos.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(stem) = path.file_stem() {
        let candidate = macos.join(stem);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let files: Vec<PathBuf> = std::fs::read_dir(&macos)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default();
    match files.as_slice() {
        [only] => Ok(only.clone()),
        _ => Err(ClapError::NoBinary(path.to_path_buf())),
    }
}

/// Reads `CFBundleExecutable` from an XML `Info.plist`. Binary plists and
/// anything unexpected return `None` and the caller falls back to guessing.
fn bundle_executable(plist: &Path) -> Option<String> {
    let bytes = std::fs::read(plist).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let after_key = text.split_once("<key>CFBundleExecutable</key>")?.1;
    let after_open = after_key.split_once("<string>")?.1;
    let (value, _) = after_open.split_once("</string>")?;
    let value = value.trim();
    // The name becomes a path component: refuse anything that could escape.
    if value.is_empty() || value.contains(['/', '\\']) || value == ".." {
        return None;
    }
    Some(value.to_string())
}

/// A loaded and initialised CLAP library.
pub(crate) struct ModuleInner {
    entry: *const clap_plugin_entry,
    factory: *const clap_plugin_factory,
    path: PathBuf,
    // Kept mapped for the life of the process, see the module docs. The
    // wrapper stops `libloading` from calling `dlclose` when this drops.
    _library: ManuallyDrop<libloading::Library>,
}

// SAFETY: CLAP requires the entry and factory functions to be thread-safe,
// and the pointers are immutable for the life of the library.
unsafe impl Send for ModuleInner {}
unsafe impl Sync for ModuleInner {}

impl ModuleInner {
    fn load(path: &Path) -> Result<ModuleInner, ClapError> {
        let binary = resolve_binary(path)?;
        // SAFETY: loading a plugin runs its initialisers. That is the point;
        // the scanner process exists to contain a plugin that misbehaves.
        let library =
            unsafe { libloading::Library::new(&binary) }.map_err(|e| ClapError::Load {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
        // `clap_entry` is a static, so the symbol is the address of the
        // struct itself.
        // SAFETY: the symbol type is a plain pointer, matching the export.
        let entry: *const clap_plugin_entry = unsafe {
            library
                .get::<*const clap_plugin_entry>(b"clap_entry\0")
                .map(|sym| *sym)
                .map_err(|_| ClapError::NoEntry(path.to_path_buf()))?
        };
        if entry.is_null() {
            return Err(ClapError::NoEntry(path.to_path_buf()));
        }
        // SAFETY: non-null and points at the plugin's static entry struct,
        // which lives as long as the library stays loaded.
        let entry_ref = unsafe { &*entry };
        if !clap_version_is_compatible(entry_ref.clap_version) {
            return Err(ClapError::IncompatibleVersion {
                path: path.to_path_buf(),
                found: version_string(entry_ref.clap_version),
                supported: version_string(CLAP_VERSION),
            });
        }
        let (Some(init), Some(get_factory)) = (entry_ref.init, entry_ref.get_factory) else {
            return Err(ClapError::NoEntry(path.to_path_buf()));
        };
        let c_path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| ClapError::NoBinary(path.to_path_buf()))?;
        let initialised = {
            let _main = RoleScope::main();
            // SAFETY: `c_path` is a valid C string for the call.
            unsafe { init(c_path.as_ptr()) }
        };
        if !initialised {
            return Err(ClapError::InitFailed(path.to_path_buf()));
        }
        // From here on `Drop` owns the matching `deinit`.
        let mut module = ModuleInner {
            entry,
            factory: std::ptr::null(),
            path: path.to_path_buf(),
            _library: ManuallyDrop::new(library),
        };
        // SAFETY: the id is a valid C string and the entry is initialised.
        let factory =
            unsafe { get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) } as *const clap_plugin_factory;
        if factory.is_null() {
            return Err(ClapError::NoFactory(path.to_path_buf()));
        }
        module.factory = factory;
        Ok(module)
    }

    fn factory(&self) -> &clap_plugin_factory {
        // SAFETY: checked non-null in `load`; valid until `deinit`.
        unsafe { &*self.factory }
    }
}

impl Drop for ModuleInner {
    fn drop(&mut self) {
        // SAFETY: `entry` points into the library, which is never unloaded.
        let entry = unsafe { &*self.entry };
        if let Some(deinit) = entry.deinit {
            let _main = RoleScope::main();
            // SAFETY: matches the successful `init` in `load`.
            unsafe { deinit() };
        }
    }
}

type Registry = Vec<(PathBuf, Arc<ModuleInner>)>;

static REGISTRY: Mutex<Registry> = Mutex::new(Vec::new());

fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A counted reference to a loaded CLAP library.
pub(crate) struct ModuleHandle {
    inner: Option<Arc<ModuleInner>>,
    key: PathBuf,
}

impl ModuleHandle {
    /// Loads `path`, or joins the existing load of the same file.
    pub(crate) fn open(path: &Path) -> Result<ModuleHandle, ClapError> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut reg = registry();
        if let Some((_, module)) = reg.iter().find(|(k, _)| *k == key) {
            return Ok(ModuleHandle {
                inner: Some(Arc::clone(module)),
                key,
            });
        }
        let module = Arc::new(ModuleInner::load(path)?);
        reg.push((key.clone(), Arc::clone(&module)));
        Ok(ModuleHandle {
            inner: Some(module),
            key,
        })
    }

    fn module(&self) -> &ModuleInner {
        // `inner` is only `None` while dropping.
        self.inner
            .as_deref()
            .expect("module handle used after drop")
    }

    pub(crate) fn path(&self) -> &Path {
        &self.module().path
    }

    pub(crate) fn plugin_count(&self) -> u32 {
        let factory = self.module().factory();
        match factory.get_plugin_count {
            // SAFETY: valid factory pointer, thread-safe by contract.
            Some(f) => unsafe { f(self.module().factory) },
            None => 0,
        }
    }

    /// The descriptor at `index`. It stays valid while the library is loaded,
    /// which the borrow of `self` guarantees.
    pub(crate) fn descriptor(&self, index: u32) -> Option<&clap_plugin_descriptor> {
        let module = self.module();
        let get = module.factory().get_plugin_descriptor?;
        // SAFETY: valid factory pointer.
        let desc = unsafe { get(module.factory, index) };
        // SAFETY: the pointer is null or a descriptor owned by the library.
        unsafe { desc.as_ref() }
    }

    /// Creates (but does not initialise) a plugin. Null when the factory
    /// refuses.
    pub(crate) fn create_plugin(&self, host: *const clap_host, id: &CStr) -> *const clap_plugin {
        let module = self.module();
        let Some(create) = module.factory().create_plugin else {
            return std::ptr::null();
        };
        let _main = RoleScope::main();
        // SAFETY: `host` outlives the plugin (the caller keeps it boxed next
        // to the plugin) and `id` is a valid C string.
        unsafe { create(module.factory, host, id.as_ptr()) }
    }
}

impl Clone for ModuleHandle {
    fn clone(&self) -> ModuleHandle {
        let _reg = registry();
        ModuleHandle {
            inner: self.inner.clone(),
            key: self.key.clone(),
        }
    }
}

impl Drop for ModuleHandle {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        let mut reg = registry();
        // The registry and this handle hold two references when this is the
        // last handle. Counts only change under the lock, so the check is
        // exact.
        if Arc::strong_count(&inner) == 2 {
            reg.retain(|(k, _)| *k != self.key);
        }
        // Dropping the final reference runs `deinit` and unloads the library
        // while the lock is held.
        drop(inner);
        drop(reg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_executable_is_extracted() {
        let dir = std::env::temp_dir().join(format!("peel-clap-plist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plist = dir.join("Info.plist");
        std::fs::write(
            &plist,
            "<plist><dict>\n<key>CFBundleName</key><string>x</string>\n\
             <key>CFBundleExecutable</key>\n  <string>Synth One</string>\n</dict></plist>",
        )
        .unwrap();
        assert_eq!(bundle_executable(&plist).as_deref(), Some("Synth One"));
        std::fs::write(
            &plist,
            "<key>CFBundleExecutable</key><string>../evil</string>",
        )
        .unwrap();
        assert_eq!(bundle_executable(&plist), None);
        std::fs::write(&plist, b"bplist00\x00\x01").unwrap();
        assert_eq!(bundle_executable(&plist), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_an_error() {
        let err = ModuleHandle::open(Path::new("/definitely/not/here.clap"))
            .err()
            .expect("must fail");
        assert!(matches!(err, ClapError::Io { .. }), "{err}");
    }
}
