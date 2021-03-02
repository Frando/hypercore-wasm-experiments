use futures::channel::mpsc;
use futures::stream::StreamExt;
use log::*;
use wasm_bindgen::prelude::*;

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

    let window = web_sys::window().expect("no global `window` exists");
    let document = window.document().expect("should have a document on window");
    let body = document.body().expect("document should have a body");

    info!("start: address {} key {}", &addr, &key);

    let (ws_send_tx, ws_send_rx) = mpsc::unbounded();
    let (ws_recv_tx, ws_recv_rx) = mpsc::unbounded();
    let (app_tx, mut app_rx) = mpsc::unbounded();

    ws::connect(ws_recv_tx, ws_send_rx, addr)?;
    hypercore::open(ws_recv_rx, ws_send_tx, app_tx, key).await?;

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
