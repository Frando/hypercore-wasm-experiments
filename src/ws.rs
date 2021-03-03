use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use futures::ready;
use futures::stream::{IntoAsyncRead, StreamExt, TryStreamExt};
use futures::{AsyncRead, AsyncWrite};
use js_sys::{ArrayBuffer, Uint8Array};
use log::*;
use std::fmt;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{BinaryType, ErrorEvent, MessageEvent, WebSocket};

pub struct WebsocketStream {
    read_half: ReadHalf,
    write_half: WriteHalf,
}

impl WebsocketStream {
    pub async fn connect(addr: impl ToString) -> io::Result<Self> {
        let addr = addr.to_string();

        let ws = WebSocket::new(&addr).map_err(into_io_error)?;
        ws.set_binary_type(BinaryType::Arraybuffer);

        let onerror_callback = Closure::wrap(Box::new(move |e: ErrorEvent| {
            error!("Websocket error: {:?}", e);
        }) as Box<dyn FnMut(ErrorEvent)>);
        ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
        onerror_callback.forget();

        // Open a oneshot channel to signal that the Websocket is open.
        let (open_tx, open_rx) = oneshot::channel();
        // Wrap in the sender in Option so it is reusable in the FnMut closure.
        let mut open_tx = Some(open_tx);
        // Create onopen callback.
        let onopen_closure: Box<dyn FnMut(JsValue)> = Box::new(move |_| {
            open_tx.take().unwrap().send(true).unwrap();
        });
        let onopen_closure = Closure::wrap(onopen_closure);
        ws.set_onopen(Some(onopen_closure.as_ref().unchecked_ref()));
        onopen_closure.forget();

        open_rx.await.unwrap();
        info!("Websocket opened on address {}", &addr);

        let read_half = ReadHalf::new(ws.clone());
        let write_half = WriteHalf::new(ws);
        Ok(Self {
            read_half,
            write_half,
        })
    }

    pub fn split(self) -> (ReadHalf, WriteHalf) {
        (self.read_half, self.write_half)
    }
}

pub struct WriteHalf {
    send_tx: mpsc::UnboundedSender<(Vec<u8>, oneshot::Sender<io::Result<()>>)>,
    signal_rx: Option<oneshot::Receiver<io::Result<()>>>,
}

impl WriteHalf {
    pub fn new(socket: WebSocket) -> Self {
        let (send_tx, mut send_rx) =
            mpsc::unbounded::<(Vec<u8>, oneshot::Sender<io::Result<()>>)>();
        spawn_local(async move {
            while let Some((message, signal_tx)) = send_rx.next().await {
                let res = socket
                    .send_with_u8_array(&message)
                    .map_err(into_io_error)
                    .map(|_| ());
                signal_tx.send(res).unwrap();
            }
        });
        Self {
            send_tx,
            signal_rx: None,
        }
    }

    pub async fn _send(&self, message: &[u8]) -> io::Result<usize> {
        let (signal_tx, signal_rx) = oneshot::channel();
        self.send_tx
            .unbounded_send((message.to_vec(), signal_tx))
            .map_err(into_io_error)?;
        signal_rx
            .await
            .map(|_| message.len())
            .map_err(into_io_error)
    }
}

impl AsyncWrite for WriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if let Some(ref mut signal_rx) = self.signal_rx {
                let res = ready!(Pin::new(signal_rx).poll(cx));
                let res = res.map(|_| buf.len()).map_err(into_io_error);
                self.signal_rx = None;
                return Poll::Ready(res);
            } else {
                let (signal_tx, signal_rx) = oneshot::channel();
                self.send_tx
                    .unbounded_send((buf.to_vec(), signal_tx))
                    .map_err(into_io_error)?;
                self.signal_rx = Some(signal_rx)
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub struct ReadHalf {
    reader: IntoAsyncRead<mpsc::UnboundedReceiver<io::Result<Vec<u8>>>>,
}

impl ReadHalf {
    fn new(socket: WebSocket) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::unbounded();
        // Create onmessage callback.
        let onmessage_callback = Closure::wrap(Box::new({
            let inbound_tx = inbound_tx.clone();
            move |e: MessageEvent| {
                let message = js_value_to_vec_u8(e.data());
                match message {
                    Ok(message) => {
                        // debug!("RECV {} {:?}", message.len(), message);
                        inbound_tx.unbounded_send(Ok(message)).unwrap();
                    }
                    Err(e) => error!("Could not cast websocket message to bytes: {:?}", e),
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        // Set message event handler on WebSocket.
        socket.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
        // Forget the callback to keep it alive.
        onmessage_callback.forget();
        let reader = inbound_rx.into_async_read();
        Self { reader }
    }
}

// impl Stream for ReadHalf {
//     type Item = io::Result<Vec<u8>>;
//     fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
//         let message = ready!(Pin::new(&mut self.inbound_rx).poll_next(cx));
//         Poll::Ready(message.map(|m| Ok(m)))
//     }
// }

impl AsyncRead for ReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.reader).poll_read(cx, buf)
    }
}

fn js_value_to_vec_u8(value: JsValue) -> Result<Vec<u8>, JsValue> {
    let buffer: ArrayBuffer = value.dyn_into()?;
    let uint8array: Uint8Array = js_sys::Uint8Array::new(&buffer);
    let mut data: Vec<u8> = vec![0; uint8array.length() as usize];
    uint8array.copy_to(data.as_mut_slice());
    Ok(data)
}

fn into_io_error<T: fmt::Debug>(value: T) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("Error: {:?}", value))
}
