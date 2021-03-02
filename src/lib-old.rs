use wasm_bindgen::prelude::*;

use wasm_bindgen::JsCast;
use web_sys::{ErrorEvent, MessageEvent, WebSocket};

use hypercore_protocol::schema::*;
use hypercore_protocol::{discovery_key, ProtocolBuilder};
use hypercore_protocol::{Channel, ChannelHandler, StreamContext, StreamHandler};
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::io::Cursor;
use log::*;
use pretty_hash::fmt as pretty_fmt;
use std::io::Result as IoResult;
// use std::io::Result;

mod utils;

// When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);
}

#[wasm_bindgen]
pub fn greet() {
    alert("Hello, hypercore-rs-wasm!");
}

#[wasm_bindgen]
pub fn open2(foo: String) -> std::result::Result<JsValue, JsValue> {
    match foo.as_str() {
        "ok" => Ok(JsValue::from_str("ok fine")),
        _ => Err(js_sys::Error::new("oh no").into()),
    }
}

#[wasm_bindgen]
pub fn start_websocket() -> Result<(), JsValue> {
    debug!("ws example start");
    // Connect to an echo server
    let ws = WebSocket::new("ws://localhost:9000")?;

    // create callback
    let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
        // handle message
        let response = e
            .data()
            .as_string()
            .expect("Can't convert received data to a string");
        debug!("message event, received data: {:?}", response);
    }) as Box<dyn FnMut(MessageEvent)>);
    // set message event handler on WebSocket
    ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
    // forget the callback to keep it alive
    onmessage_callback.forget();

    let onerror_callback = Closure::wrap(Box::new(move |e: ErrorEvent| {
        debug!("error event: {:?}", e);
    }) as Box<dyn FnMut(ErrorEvent)>);
    ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
    onerror_callback.forget();

    let cloned_ws = ws.clone();
    let onopen_callback = Closure::wrap(Box::new(move |_| {
        debug!("socket opened");
        match cloned_ws.send_with_str("ping") {
            Ok(_) => debug!("message successfully sent"),
            Err(err) => debug!("error sending message: {:?}", err),
        }
    }) as Box<dyn FnMut(JsValue)>);
    ws.set_onopen(Some(onopen_callback.as_ref().unchecked_ref()));
    onopen_callback.forget();

    Ok(())
}

#[wasm_bindgen]
pub async fn open() -> std::result::Result<JsValue, JsValue> {
    utils::set_panic_hook();
    console_log::init_with_level(log::Level::Debug).unwrap();
    let is_initiator = true;
    info!("START is_initiator {}", is_initiator);
    let mut feedstore = FeedStore::new();
    let feedstore = Arc::new(feedstore);
    let mut reader = Cursor::new(vec![0u8; 8]);
    let mut writer = Cursor::new(vec![0u8; 8]);
    let mut protocol = ProtocolBuilder::new(is_initiator)
        .set_handlers(feedstore)
        .from_rw(reader, writer);

    protocol.listen().await.map_err(map_err)?;
    Ok(JsValue::null())
}

fn map_err(err: impl std::error::Error) -> JsValue {
    js_sys::Error::new(&format!("{}", err)).into()
}

struct FeedStore {
    feeds: HashMap<String, Arc<Feed>>,
}
impl FeedStore {
    pub fn new() -> Self {
        let feeds = HashMap::new();
        Self { feeds }
    }

    pub fn add(&mut self, feed: Feed) {
        let hdkey = hex::encode(&feed.discovery_key);
        self.feeds.insert(hdkey, Arc::new(feed));
    }

    pub fn get(&self, discovery_key: &[u8]) -> Option<&Arc<Feed>> {
        let hdkey = hex::encode(discovery_key);
        self.feeds.get(&hdkey)
    }
}

/// We implement the StreamHandler trait on the FeedStore.
#[async_trait]
impl StreamHandler for FeedStore {
    async fn on_discoverykey(
        &self,
        protocol: &mut StreamContext,
        discovery_key: &[u8],
    ) -> IoResult<()> {
        log::trace!("open discovery_key: {}", pretty_fmt(discovery_key).unwrap());
        if let Some(feed) = self.get(discovery_key) {
            protocol.open(feed.key.clone(), feed.clone()).await
        } else {
            Ok(())
        }
    }
}

/// A Feed is a single unit of replication, an append-only log.
/// This toy feed can only read sequentially and does not save or buffer anything.
#[derive(Debug)]
struct Feed {
    key: Vec<u8>,
    discovery_key: Vec<u8>,
    state: FeedState,
}
impl Feed {
    pub fn new(key: Vec<u8>) -> Self {
        Feed {
            discovery_key: discovery_key(&key),
            key,
            state: FeedState::default(),
        }
    }
}

/// The Feed structs implements the ChannelHandler trait.
/// This allows to pass a Feed struct into the protocol when `open`ing a channel,
/// making it the handler for all messages that arrive on this channel.
/// The trait fns all receive a `channel` arg that allows to send messages over
/// the current channel.
#[async_trait]
impl ChannelHandler for Feed {
    async fn on_open<'a>(&self, channel: &mut Channel<'a>, discovery_key: &[u8]) -> IoResult<()> {
        log::info!("open channel {}", pretty_fmt(&discovery_key).unwrap());
        let msg = Want {
            start: 0,
            length: None,
        };
        channel.want(msg).await
    }

    async fn on_have<'a>(&self, channel: &mut Channel<'a>, msg: Have) -> IoResult<()> {
        // let state = &mut self.state;
        // // Check if the remote announces a new head.
        // log::info!("receive have: {} (state {})", msg.start, state.remote_head);
        // if msg.start > state.remote_head {
        //     // Store the new remote head.
        //     state.remote_head = msg.start;
        //     // If we didn't start reading, request first data block.
        //     if !state.started {
        //         state.started = true;
        //         let msg = Request {
        //             index: 0,
        //             bytes: None,
        //             hash: None,
        //             nodes: None,
        //         };
        //         channel.request(msg).await?;
        //     }
        // }
        Ok(())
    }

    async fn on_data<'a>(&self, channel: &mut Channel<'a>, msg: Data) -> IoResult<()> {
        let state = &self.state;
        log::info!(
            "receive data: idx {}, {} bytes (remote_head {})",
            msg.index,
            msg.value.as_ref().map_or(0, |v| v.len()),
            state.remote_head
        );

        // if let Some(value) = msg.value {
        //     let mut stdout = io::stdout();
        //     stdout.write_all(&value).await?;
        //     stdout.flush().await?;
        // }

        let next = msg.index + 1;
        if state.remote_head >= next {
            // Request next data block.
            let msg = Request {
                index: next,
                bytes: None,
                hash: None,
                nodes: None,
            };
            channel.request(msg).await?;
        }

        Ok(())
    }
}

/// A FeedState stores the head seq of the remote.
/// This would have a bitfield to support sparse sync in the actual impl.
#[derive(Debug)]
struct FeedState {
    remote_head: u64,
    started: bool,
}
impl Default for FeedState {
    fn default() -> Self {
        FeedState {
            remote_head: 0,
            started: false,
        }
    }
}
