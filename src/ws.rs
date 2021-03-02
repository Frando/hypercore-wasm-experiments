use futures::channel::mpsc::{UnboundedReceiver as Receiver, UnboundedSender as Sender};
use futures::stream::StreamExt;
use js_sys::{ArrayBuffer, Uint8Array};
use log::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{BinaryType, ErrorEvent, MessageEvent, WebSocket};

pub fn connect(
    ws_recv_tx: Sender<Vec<u8>>,
    mut ws_send_rx: Receiver<Vec<u8>>,
    addr: impl ToString,
) -> Result<(), JsValue> {
    let addr = addr.to_string();

    let ws = WebSocket::new(&addr)?;
    ws.set_binary_type(BinaryType::Arraybuffer);

    // Create onmessage callback.
    let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
        let message = js_value_to_vec_u8(e.data());
        match message {
            Ok(message) => {
                // debug!("RECV {} {:?}", message.len(), message);
                ws_recv_tx.unbounded_send(message).unwrap();
            }
            Err(e) => error!("Could not cast websocket message to bytes: {:?}", e),
        }
    }) as Box<dyn FnMut(MessageEvent)>);
    // Set message event handler on WebSocket.
    ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
    // Forget the callback to keep it alive.
    onmessage_callback.forget();

    let onerror_callback = Closure::wrap(Box::new(move |e: ErrorEvent| {
        error!("Websocket error: {:?}", e);
    }) as Box<dyn FnMut(ErrorEvent)>);
    ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
    onerror_callback.forget();

    // Open a oneshot channel to signal that the Websocket is open.
    let (open_tx, open_rx) = futures::channel::oneshot::channel();

    // Create onopen callback.
    let mut open_tx = Some(open_tx);
    let onopen_closure: Box<dyn FnMut(JsValue)> = Box::new(move |_| {
        info!("Websocket opened on address {}", &addr);
        open_tx.take().unwrap().send(true).unwrap();
    });
    let onopen_closure = Closure::wrap(onopen_closure);
    ws.set_onopen(Some(onopen_closure.as_ref().unchecked_ref()));
    onopen_closure.forget();

    // Open send loop.
    let cloned_ws = ws.clone();
    spawn_local(async move {
        open_rx.await.unwrap();
        loop {
            let message = ws_send_rx.next().await;
            if let Some(message) = message {
                match cloned_ws.send_with_u8_array(&message) {
                    Ok(_) => {
                        // debug!("message successfully sent (len {})", message.len()),
                    }
                    Err(err) => error!("Websocket error while sending message: {:?}", err),
                }
            } else {
                debug!("ws_send_rx channel ended");
                break;
            }
        }
    });

    Ok(())
}

fn js_value_to_vec_u8(value: JsValue) -> Result<Vec<u8>, JsValue> {
    let buffer: ArrayBuffer = value.dyn_into()?;
    let uint8array: Uint8Array = js_sys::Uint8Array::new(&buffer);
    let mut data: Vec<u8> = vec![0; uint8array.length() as usize];
    uint8array.copy_to(data.as_mut_slice());
    Ok(data)
}
