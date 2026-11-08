//! `window.pdfcraft`: the page command API (`craft-web-api`), over the app's UI control channel
//! (`pdfcraft_ui_egui::control`, the same one `--control` serves on the desktop).
//!
//! ```js
//! await pdfcraft.docInspect()                       // ui.state: documents, page, zoom, mode, panels
//! await pdfcraft.commands()                         // ui.commands: the registry commands
//! await pdfcraft.request("ui.command", {id})        // run one, as the menu would
//! await pdfcraft.request("ui.type", {text: "…"})    // and the other ui.* methods (click, key, drag, set)
//! ```
//!
//! PdfCraft's commands take no parameters (`ui.command {id}`), so there is no `execute`.
//! Installed only when the page loads the app with `?control`.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};

use craft_web_api::methods::Methods;
use craft_web_api::page::{PageApi, Request};
use pdfcraft_ui_egui::control::{ControlClient, Reply};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// How often a waiting call looks for the app's reply, in milliseconds (about a frame).
const POLL_MS: i32 = 16;

/// The control methods behind each `window.pdfcraft` function.
const METHODS: Methods = Methods {
    commands: Some("ui.commands"),
    execute: None,
    doc_inspect: Some("ui.state"),
    ui_inspect: Some("ui.inspect"),
    screenshot: Some("ui.screenshot"),
};

/// Install `window.pdfcraft` over `client` (from `PdfCraftApp::attach_control`).
pub fn install(client: ControlClient) -> Result<(), JsValue> {
    let request: Request = Rc::new(move |method, params| send(&client, &method, params));
    Rc::new(PageApi::new("pdfcraft", METHODS, request)).install()
}

/// Queue a request and wait, without blocking the frame, for the app's reply as the protocol's
/// `{ok, result}` / `{ok: false, error}` object.
fn send(client: &ControlClient, method: &str, params: Value) -> Pin<Box<dyn Future<Output = Value>>> {
    let reply = client.send(method, params);
    Box::pin(async move {
        loop {
            if let Some(v) = poll(&reply) {
                return v;
            }
            sleep(POLL_MS).await;
        }
    })
}

fn poll(reply: &Receiver<Reply>) -> Option<Value> {
    match reply.try_recv() {
        Ok(Ok(result)) => Some(json!({"ok": true, "result": result})),
        Ok(Err(error)) => Some(json!({"ok": false, "error": error})),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(json!({"ok": false, "error": "request dropped"})),
    }
}

/// Yield to the page's event loop for `ms` milliseconds.
async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = JsFuture::from(promise).await;
}
