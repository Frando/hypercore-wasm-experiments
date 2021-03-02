use futures::channel::mpsc;
use log::*;
use wasm_bindgen::prelude::*;

mod hypercore;
mod utils;
mod ws;

#[wasm_bindgen]
pub async fn run_async(addr: String, key: String) -> Result<(), JsValue> {
    utils::set_panic_hook();
    console_log::init_with_level(log::Level::Debug).unwrap();

    info!("start: address {} key {}", &addr, &key);

    let (ws_send_tx, ws_send_rx) = mpsc::unbounded();
    let (ws_recv_tx, ws_recv_rx) = mpsc::unbounded();

    ws::connect(ws_recv_tx, ws_send_rx, addr)?;
    hypercore::open(ws_recv_rx, ws_send_tx, key).await?;
    Ok(())
}
