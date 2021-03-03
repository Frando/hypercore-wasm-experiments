use futures::channel::mpsc;
use futures::stream::StreamExt;
use hypercore_protocol::ProtocolBuilder;
use log::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Document, HtmlElement, Window};

mod hypercore;
mod utils;
mod ws;

pub enum AppEvent {
    ContentLoaded(String),
}

#[wasm_bindgen]
pub async fn run_async(addr: String, key: String) -> Result<(), JsValue> {
    utils::set_panic_hook();
    console_log::init_with_level(log::Level::Debug).unwrap();

    info!("start: websocket address {}, hypercore key {}", &addr, &key);

    let websocket = ws::WebsocketStream::connect(addr).await.unwrap();
    let (reader, writer) = websocket.split();
    let proto = ProtocolBuilder::new(true).connect_rw(reader, writer);

    let (app_tx, mut app_rx) = mpsc::unbounded();

    spawn_local(async move {
        hypercore::replicate(proto, key, app_tx).await.unwrap();
    });

    let (_window, document, body) = get_elements().unwrap();
    while let Some(event) = app_rx.next().await {
        match event {
            AppEvent::ContentLoaded(content) => {
                // Manufacture the element we're gonna append
                let val = document.create_element("pre")?;
                val.set_text_content(Some(&content));
                body.append_child(&val)?;
            }
        }
    }
    Ok(())
}

fn get_elements() -> Option<(Window, Document, HtmlElement)> {
    let window = web_sys::window().expect("no global `window` exists");
    let document = window.document().expect("should have a document on window");
    let body = document.body().expect("document should have a body");
    Some((window, document, body))
}
