//! Hosted in Clawku (`?host=parent&open=host`): Clawku owns the file list, New, Open, Close,
//! Rename, Share, versions, sign-in, the user's name and help, and the app shows the one document
//! Clawku sent. [`PdfCraftApp::hosted`] is the one flag; this module says what it hides, and the
//! menus, palette, tool panels, hotbar and tab strip ask it. Unhosted (the standalone web build,
//! the desktop apps) nothing changes, apart from the few commands that can't work in a browser.

use crate::PdfCraftApp;

/// File ▸ Share, a menu-only item shown only when hosted: Clawku runs sharing.
pub(crate) const SHARE: &str = "file.share";

/// Commands hidden when hosted: ways to make or show another document or an empty app (Clawku
/// makes and opens files), the recent list, closing the document, tab switching, and the
/// standalone app's web pages and update check.
const HIDDEN_WHEN_HOSTED: &[&str] = &[
    "create.blank",
    "create.clipboard",
    "file.open_recent",
    "file.clear_recent",
    crate::menus::NO_RECENT,
    "file.pin_folder",
    "file.close",
    "file.close_all",
    "view.next_tab",
    "view.previous_tab",
    crate::menus::DOCUMENT,
    "help.app_page",
    "help.github",
    "help.website",
    "help.discord",
    "help.check_updates",
    // Dead in a browser: they only say the web build can't do it yet.
    "create.images",
    "edit.attach",
    "edit.image",
    "comment.import",
    "form.import_data",
    "form.merge_data",
];

/// Commands that can't work in a browser (no folders to pin, no certificate store to sign with),
/// hidden in every web build rather than left as dead items.
const HIDDEN_ON_WEB: &[&str] = &["file.pin_folder", "sign.digital", "sign.certify", "sign.certify_invisible"];

impl PdfCraftApp {
    /// Whether the menus, palette, tool panels and hotbar leave `id` out, and running it does
    /// nothing: hidden when hosted, or unusable in a browser. File ▸ Share shows only when hosted.
    pub(crate) fn hides_command(&self, id: &str) -> bool {
        if id == SHARE {
            return !self.hosted;
        }
        (self.hosted && HIDDEN_WHEN_HOSTED.contains(&id)) || (cfg!(target_arch = "wasm32") && HIDDEN_ON_WEB.contains(&id))
    }

    /// File ▸ Share: the host page shares the document (`pdfcraft:share`).
    pub(crate) fn share(&mut self) {
        if let Some(share) = self.host_share.as_mut() {
            share();
        }
    }

    /// Hosted: a result the app made (Combine, Create, Extract, Summarize comments) goes to the
    /// host page as a new file next to the document instead of opening as a tab Clawku doesn't
    /// know about. `false` when not hosted (the caller opens it as a tab).
    pub(crate) fn created_to_host(&mut self, name: &str, bytes: &[u8]) -> bool {
        if !self.hosted {
            return false;
        }
        match crate::editing::send_copy(name, bytes) {
            Some(Ok(())) => {
                self.notify_fmt("Saved {name} as a new file", &[("name", name)]);
                true
            }
            Some(Err(e)) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", name), ("e", &e)]);
                true
            }
            None => false,
        }
    }

    /// Hosted: keep the names of the open documents where copies sent to the host can see them,
    /// so a copy never overwrites the document (`editing::copy_name`).
    pub(crate) fn note_hosted_names(&self) {
        if !crate::editing::copies_to_host() {
            return;
        }
        let names = self.views.iter().filter_map(|v| self.session.get(v.id).map(|d| d.name.clone())).collect();
        crate::editing::note_open_names(names);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::{PdfCraftApp, SaveTarget};

    const PDF: &[u8] = include_bytes!("../tests/data/form.pdf");

    type Sent = Rc<RefCell<Vec<(String, usize, bool)>>>;

    /// A recorder standing in for the host page's `pdfcraft:save`.
    fn recorder() -> (Sent, crate::HostSaveFn) {
        let sent: Sent = Rc::default();
        let log = sent.clone();
        (
            sent,
            Box::new(move |name: &str, bytes: &[u8], save_as: bool| {
                log.borrow_mut().push((name.to_string(), bytes.len(), save_as));
                Ok(())
            }),
        )
    }

    fn hosted_app() -> (PdfCraftApp, Sent, Sent) {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.hosted = true;
        let (saves, save) = recorder();
        let (copies, copy) = recorder();
        app.host_save = Some(save);
        crate::editing::set_host_copies(Some(copy));
        app.open_bytes("Contract.pdf", None, PDF.to_vec()).unwrap();
        (app, saves, copies)
    }

    fn harness(hosted: bool) -> Harness<'static, PdfCraftApp> {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.hosted = hosted;
            app
        });
        h.run_steps(4);
        h
    }

    #[test]
    fn save_overwrites_the_hosts_file_and_save_as_lets_the_host_name_it() {
        let (mut app, saves, copies) = hosted_app();
        assert!(app.save_active(SaveTarget::InPlace));
        assert!(app.save_active(SaveTarget::As));
        let saves = saves.borrow();
        assert_eq!(saves.iter().map(|(n, _, a)| (n.as_str(), *a)).collect::<Vec<_>>(), [("Contract.pdf", false), ("Contract.pdf", true)]);
        assert!(saves.iter().all(|(_, len, _)| *len > 0));
        assert!(copies.borrow().is_empty(), "Save is no copy");
        crate::editing::set_host_copies(None);
    }

    #[test]
    fn results_go_to_the_host_as_new_files_under_names_no_document_has() {
        let (mut app, _, copies) = hosted_app();
        app.note_hosted_names();
        // The document's own name comes back as a copy's name, never the same.
        app.open_created("Contract.pdf", std::sync::Arc::new(PDF.to_vec()), "Created");
        app.open_created("Combined.pdf", std::sync::Arc::new(PDF.to_vec()), "Created");
        assert_eq!(app.views.len(), 1, "no untracked tab opens");
        let copies = copies.borrow();
        assert_eq!(copies.iter().map(|(n, _, a)| (n.as_str(), *a)).collect::<Vec<_>>(), [("Contract (copy).pdf", false), ("Combined.pdf", false)]);
        crate::editing::set_host_copies(None);
    }

    #[test]
    fn copy_names_step_past_every_open_document() {
        let taken = ["a.pdf".to_string(), "A (copy).PDF".to_string()];
        assert_eq!(crate::editing::copy_name("b.pdf", &taken), "b.pdf");
        assert_eq!(crate::editing::copy_name("a.pdf", &taken), "a (copy 2).pdf");
        assert_eq!(crate::editing::copy_name("notes", &["notes".to_string()]), "notes (copy)");
    }

    #[test]
    fn hosted_hides_new_close_recent_and_the_standalone_links() {
        let (app, _, _) = hosted_app();
        crate::editing::set_host_copies(None);
        let ctx = egui::Context::default();
        let bar = craft_menu::layout(&app.menu_bar(&ctx), craft_menu::Target::Linux, &crate::menus::layout_options()).bar;
        for id in
            ["create.blank", "file.open_recent", "file.close", "file.close_all", "help.github", "help.discord", "help.check_updates", "view.next_tab"]
        {
            assert!(bar.find(&craft_menu::CommandRef::new(id)).is_none(), "{id} is hidden when hosted");
        }
        for id in ["file.open", "file.save", "file.save_as", "print.dialog", "help.about", super::SHARE] {
            assert!(bar.find(&craft_menu::CommandRef::new(id)).is_some(), "{id} stays");
        }
        // Not hosted: the menus are as they were, without Share.
        let mut plain = PdfCraftApp::new();
        plain.set_option("language", "en").unwrap();
        let bar = craft_menu::layout(&plain.menu_bar(&ctx), craft_menu::Target::Linux, &crate::menus::layout_options()).bar;
        assert!(bar.find(&craft_menu::CommandRef::new("create.blank")).is_some());
        assert!(bar.find(&craft_menu::CommandRef::new("file.close")).is_some());
        assert!(bar.find(&craft_menu::CommandRef::new(super::SHARE)).is_none());
    }

    #[test]
    fn hidden_commands_do_not_run_from_shortcuts_or_scripts() {
        let (mut app, _, _) = hosted_app();
        crate::editing::set_host_copies(None);
        assert!(!app.execute("file.close"));
        assert!(!app.execute("create.blank"));
        assert_eq!(app.views.len(), 1, "the document stays the only one");
    }

    #[test]
    fn share_asks_the_host() {
        let (mut app, _, _) = hosted_app();
        crate::editing::set_host_copies(None);
        let asked = Rc::new(RefCell::new(0));
        let count = asked.clone();
        app.host_share = Some(Box::new(move || *count.borrow_mut() += 1));
        let ctx = egui::Context::default();
        app.run_menu_command(&ctx, &craft_menu::CommandRef::new(super::SHARE));
        assert_eq!(*asked.borrow(), 1);
    }

    #[test]
    fn hosted_starts_on_no_home_screen_and_drops_home_and_community() {
        let h = harness(true);
        assert!(h.query_by_label("Welcome to ClawPDF").is_none(), "no Home screen before the host's file arrives");
        assert!(h.query_by_label("Home").is_none());
        assert!(h.query_by_label("Community").is_none());
        assert!(h.query_by_label("Share").is_some());
        let h = harness(false);
        assert!(h.query_by_label("Welcome to ClawPDF").is_some(), "the standalone app keeps its Home screen");
        assert!(h.query_by_label("Home").is_some());
        assert!(h.query_by_label("Share").is_none());
    }
}
