use super::{PdfCraftApp, StagingName, staging_suffixes};

/// A save held back by [`Dialog::OverwriteChanged`](crate::Dialog::OverwriteChanged): the file
/// on disk is another version than the document came from (#2166).
#[cfg(not(target_arch = "wasm32"))]
pub struct PendingOverwrite {
    pub id: pdfcraft_engine::DocId,
    pub dest: String,
    target: SaveTarget,
    /// What was to run once the document is written (close its tab, quit).
    after: Box<dyn FnOnce(&mut PdfCraftApp) + Send>,
}

/// Where Save writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    /// The file's own path; asks for one if the document has none (e.g. opened from bytes).
    InPlace,
    /// Always ask for a new location.
    As,
}

impl PdfCraftApp {
    /// Save the active document. Returns `true` if it was written.
    pub fn save_active(&mut self, target: SaveTarget) -> bool {
        match self.active {
            Some(i) => self.save_view(i, target),
            None => false,
        }
    }

    /// Save the document shown in tab `index`. Returns `true` if it was written now. When it has
    /// to ask where (a new document, or Save As) it returns `false` and saves on a later frame,
    /// once the user has chosen.
    pub fn save_view(&mut self, index: usize, target: SaveTarget) -> bool {
        self.save_then(index, target, |_| {})
    }

    /// [`Self::save_view`], then `after` once the document is written: now, or on a later frame
    /// when the user had to choose where. `after` never runs when the save fails or is cancelled.
    pub(crate) fn save_then(&mut self, index: usize, target: SaveTarget, after: impl FnOnce(&mut Self) + Send + 'static) -> bool {
        // What's typed in a field is part of what's saved (#166).
        if let Some(id) = self.views.get(index).map(|v| v.id)
            && !self.commit_typing_in(id)
        {
            return false;
        }
        let Some(id) = self.views.get(index).map(|v| v.id) else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let (name, path) = (doc.name.clone(), doc.path.clone());
        // An embedding page takes the saves on every target (the desktop app never sets one;
        // tests do, to check what a hosted save sends).
        #[cfg(not(target_arch = "wasm32"))]
        if self.host_save.is_some() {
            let _ = path;
            return self.save_bytes_away(index, id, &name, target, Box::new(after));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let doc_path = path.clone();
            let destination = match (target, path, &self.save_override) {
                (_, _, Some(p)) => p.clone(),
                // An email attachment's temporary copy is no place to keep work: ask, as for a new
                // document (`is_mail_temp_copy`).
                (SaveTarget::InPlace, Some(p), _) if !is_mail_temp_copy(&p) => p,
                _ => {
                    let name = if name.to_ascii_lowercase().ends_with(".pdf") { name } else { format!("{name}.pdf") };
                    let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name);
                    let dialog = in_document_folder(dialog, doc_path.as_deref());
                    // The bytes are taken once the user has chosen, so edits made meanwhile are saved too.
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, dest| {
                        app.save_doc_to_then(id, &dest.to_string_lossy(), target, Box::new(after));
                    });
                    return false;
                }
            };
            self.save_doc_to_then(id, &destination, target, Box::new(after))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            self.save_bytes_away(index, id, &name, target, Box::new(after))
        }
    }

    /// Save on the web, and wherever an embedding page takes the saves: the bytes go to the page
    /// ([`Self::host_save`], `?host=parent`) under the document's own name, with `save_as` for
    /// Save As (the page asks for the new name itself), or else the browser downloads them.
    fn save_bytes_away(
        &mut self,
        index: usize,
        id: pdfcraft_engine::DocId,
        name: &str,
        target: SaveTarget,
        after: Box<dyn FnOnce(&mut Self) + Send>,
    ) -> bool {
        if !self.flatten_fill_sign_for_save(id) {
            return false;
        }
        let bytes = match self.session.save_bytes(id) {
            Ok(b) => b,
            Err(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", name), ("e", &e.to_string())]);
                return false;
            }
        };
        let to_host = self.host_save.is_some();
        let delivered = match &self.host_save {
            Some(save) => save(name, &bytes, matches!(target, SaveTarget::As)),
            #[cfg(target_arch = "wasm32")]
            None => download(name, &bytes),
            #[cfg(not(target_arch = "wasm32"))]
            None => Err("no page to save to".to_string()),
        };
        match delivered {
            Ok(()) => {
                let _ = self.session.mark_saved(id, bytes, None);
                if let Some(doc) = self.session.get(id)
                    && let Some(view) = self.views.get_mut(index)
                {
                    view.document_changed(&doc.info);
                }
                // The host reports where the file went (and any failure) itself.
                if !to_host {
                    self.notify_fmt("Downloaded {name}", &[("name", name)]);
                }
                after(self);
                true
            }
            Err(e) if to_host => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", name), ("e", &e)]);
                false
            }
            Err(e) => {
                self.notify_fmt("Couldn't download {name}: {e}", &[("name", name), ("e", &e.to_string())]);
                false
            }
        }
    }

    /// [`Self::save_doc_to`], then `after`. When `dest` is the document's own file and it is
    /// another version on disk than the one the document came from (something else wrote it
    /// since the open or the last save, #2166), it asks first: the save waits in
    /// `pending_overwrite` behind [`Dialog::OverwriteChanged`](crate::Dialog::OverwriteChanged).
    /// Returns `true` if written now.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_doc_to_then(&mut self, id: pdfcraft_engine::DocId, dest: &str, target: SaveTarget, after: Box<dyn FnOnce(&mut Self) + Send>) -> bool {
        let own = self.session.get(id).and_then(|d| d.path.as_deref()).is_some_and(|p| crate::same_file(p, dest));
        if own && self.watch.disk_differs(id, dest, self.session.get(id).map(|d| d.saved_bytes())) {
            self.pending_overwrite = Some(PendingOverwrite { id, dest: dest.to_string(), target, after });
            self.dialog = Some(crate::Dialog::OverwriteChanged);
            return false;
        }
        let saved = self.save_doc_to(id, dest, target);
        if saved {
            after(self);
        }
        saved
    }

    /// The dialog's Save anyway: write over the changed file.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn overwrite_changed_file(&mut self) {
        let Some(p) = self.pending_overwrite.take() else { return };
        if self.save_doc_to(p.id, &p.dest, p.target) {
            (p.after)(self);
        }
    }

    /// The dialog's Save as…: leave the file on disk as it is and ask where to put this version.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn save_changed_file_elsewhere(&mut self) {
        let Some(p) = self.pending_overwrite.take() else { return };
        if let Some(index) = self.views.iter().position(|v| v.id == p.id) {
            self.save_then(index, SaveTarget::As, p.after);
        }
    }

    /// Write document `id` to `dest` and make it the document's file. Returns `true` if written.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_doc_to(&mut self, id: pdfcraft_engine::DocId, dest: &str, target: SaveTarget) -> bool {
        // Save As answers on a later frame: commit what was typed meanwhile, too (#166), even
        // if another tab is active by then.
        if !self.commit_typing_in(id) || !self.flatten_fill_sign_for_save(id) {
            return false;
        }
        let Some((name, full)) = self.session.get(id).map(|d| {
            // Save As to another file is a clean copy, without the earlier revisions that still
            // hold deleted pages or replaced text (#2165). A signed document stays incremental so
            // its signatures keep covering the file.
            let full = target == SaveTarget::As
                && !d.path.as_deref().is_some_and(|p| crate::same_file(p, dest))
                && d.read_only_reason.is_none()
                && !d.is_signed();
            (d.name.clone(), full)
        }) else {
            self.notify_tr("The document was closed before it could be saved.");
            return false;
        };
        let bytes = if full { self.session.save_full_bytes(id) } else { self.session.save_bytes(id) };
        let bytes = match bytes.and_then(|b| self.session.check_save(id, &b).map(|()| b)) {
            Ok(b) => b,
            Err(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return false;
            }
        };
        if let Err(e) = write_atomically(dest, &bytes) {
            self.notify_fmt("Couldn't save {name}: {e}", &[("name", dest), ("e", &e.to_string())]);
            return false;
        }
        // Our own write is not a change on disk to reload (#431).
        self.watch.known_now(id, dest);
        match self.session.mark_saved(id, bytes, Some(dest.to_string())) {
            Ok(()) => {
                self.forget_recovery(id);
                if let Some(doc) = self.session.get(id)
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
                {
                    view.document_changed(&doc.info);
                    // Saving over a version that changed on disk keeps this one: nothing to ask.
                    view.disk_changed = false;
                }
                self.notify_fmt("Saved {name}", &[("name", &short_name(dest))]);
                #[cfg(target_os = "macos")]
                crate::macos_association::repair(std::path::Path::new(dest));
                true
            }
            Err(e) => {
                self.notify_fmt("Saved, but reopening failed: {e}", &[("e", &e.to_string())]);
                false
            }
        }
    }
}

/// Whether `path` is the temporary copy an email client opened an attachment from, so that Save
/// asks where to keep the document instead of writing to it.
///
/// Outlook opens an attachment from its own cache: `INetCache\Content.Outlook\<random>\` for
/// classic Outlook (`Temporary Internet Files\Content.Outlook\` on older Windows), and the
/// `Packages\Microsoft.OutlookForWindows_<id>` package folder for the new Outlook. The patterns
/// follow those layouts, so an ordinary folder that merely shares a name doesn't match. Saving there either fails,
/// because Outlook can still hold the file, or appears to work while the edits stay out of the
/// email and are deleted with the cache. Apple Mail opens one from `Mail Downloads/<UUID>/` in
/// its sandbox container, where other apps can't write, so saving there fails (#2751). Matched on
/// either separator and in any case, so it can be tested on every platform.
pub(crate) fn is_mail_temp_copy(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    // Outlook's cache holds `<marker><random folder>/<file>`: exactly one folder, then the file.
    let in_outlook_cache = |marker: &str| {
        p.split_once(marker)
            .is_some_and(|(_, rest)| matches!(rest.split('/').collect::<Vec<_>>()[..], [dir, file] if !dir.is_empty() && !file.is_empty()))
    };
    in_outlook_cache("/inetcache/content.outlook/")
        || in_outlook_cache("/temporary internet files/content.outlook/")
        || p.contains("/packages/microsoft.outlookforwindows_")
        || p.contains("/library/containers/com.apple.mail/data/library/mail downloads/")
}

/// The folder a Save As style dialog should open in for a document stored at `path`: the
/// document's own folder, as Acrobat and Foxit do, instead of whichever folder the system picks.
/// `None` (keep the system's choice) for a new document, a path with no parent, or an email
/// attachment's temporary copy (`is_mail_temp_copy`), which is no place to keep work. Also right
/// for UNC paths on Windows (`\\server\share\dir\f.pdf` gives `\\server\share\dir`).
pub(crate) fn save_dialog_folder(path: Option<&str>) -> Option<std::path::PathBuf> {
    let path = path.filter(|p| !is_mail_temp_copy(p))?;
    let parent = std::path::Path::new(path).parent()?;
    (!parent.as_os_str().is_empty()).then(|| parent.to_path_buf())
}

/// `dialog` opened in the folder of the document stored at `path` ([`save_dialog_folder`]).
pub(crate) fn in_document_folder(dialog: rfd::AsyncFileDialog, path: Option<&str>) -> rfd::AsyncFileDialog {
    match save_dialog_folder(path) {
        Some(dir) => dialog.set_directory(dir),
        None => dialog,
    }
}

/// Write via a temporary file in the same directory and rename over the target, so a crash or
/// full disk never leaves a half-written PDF where the original was.
pub fn write_atomically(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    write_atomically_with(path, bytes, staging_suffixes())
}

/// [`write_atomically`], trying the staging names that `suffixes` give.
pub(super) fn write_atomically_with(path: &str, bytes: &[u8], suffixes: impl IntoIterator<Item = u64>) -> std::io::Result<()> {
    write_atomically_syncing(path, bytes, suffixes, std::fs::File::sync_all)
}

/// [`write_atomically_with`], flushing the staging file with `sync_all` (the standard library's
/// in the app, a failing one in tests). The writer itself is the platform crate's, shared with
/// the tools and the CLI (#2166); the desktop app keeps its staging-name form.
pub(super) fn write_atomically_syncing(
    path: &str,
    bytes: &[u8],
    suffixes: impl IntoIterator<Item = u64>,
    sync_all: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    pdfcraft_platform::staging::write_atomically_with(std::path::Path::new(path), bytes, StagingName::TagThenSuffix, suffixes, sync_all)
}

#[cfg(not(target_arch = "wasm32"))]
fn short_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Offer bytes as a browser download.
#[cfg(target_arch = "wasm32")]
pub(crate) fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    // Hosted in Clawku: a new file next to the document instead (`set_host_copies`).
    if let Some(sent) = send_copy(name, bytes) {
        return sent;
    }
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "anchor")?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

thread_local! {
    /// Where generated files go instead of a browser download ([`set_host_copies`]).
    static HOST_COPIES: std::cell::RefCell<Option<crate::HostSaveFn>> = const { std::cell::RefCell::new(None) };
    /// The open documents' names, so a copy never takes one ([`note_open_names`]).
    static OPEN_NAMES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Clawku (`?host=parent&open=host`): every file the app makes besides the document itself
/// (exports, Print ▸ Save as PDF, Reduce size, Extract and Split, OCR and action results, data
/// and measurement exports, attachments, and the results of Combine and Create) goes to the host
/// page as a new file next to the document: `save(name, bytes, false)` under a name no open
/// document has, never as a browser download the host would not see. `None` restores downloads.
/// Set by the web app; per thread (the browser has one).
pub fn set_host_copies(save: Option<crate::HostSaveFn>) {
    HOST_COPIES.with(|c| *c.borrow_mut() = save);
}

/// Whether generated files go to the host page ([`set_host_copies`]).
pub(crate) fn copies_to_host() -> bool {
    HOST_COPIES.with(|c| c.borrow().is_some())
}

/// The names of the documents open now; a copy sent to the host gets another name.
pub(crate) fn note_open_names(names: Vec<String>) {
    OPEN_NAMES.with(|n| *n.borrow_mut() = names);
}

/// Send `bytes` to the host page as a new file ([`set_host_copies`]); `None` when there is no
/// host to send to (the caller downloads or writes instead).
pub(crate) fn send_copy(name: &str, bytes: &[u8]) -> Option<Result<(), String>> {
    let name = OPEN_NAMES.with(|n| copy_name(name, &n.borrow()));
    HOST_COPIES.with(|c| c.borrow().as_ref().map(|save| save(&name, bytes, false)))
}

/// `name`, or `name (copy).ext`, `name (copy 2).ext`… when an open document is called that
/// (ignoring case): a copy stored under the document's name would overwrite it on the host.
pub(crate) fn copy_name(name: &str, taken: &[String]) -> String {
    let free = |candidate: &str| !taken.iter().any(|t| t.to_lowercase() == candidate.to_lowercase());
    if free(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    for n in 1..1000u32 {
        let candidate = if n == 1 { format!("{stem} (copy){ext}") } else { format!("{stem} (copy {n}){ext}") };
        if free(&candidate) {
            return candidate;
        }
    }
    format!("{stem} (copy {}){ext}", taken.len().saturating_add(1000))
}
