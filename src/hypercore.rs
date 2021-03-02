use async_trait::async_trait;
use futures::channel::mpsc::{UnboundedReceiver as Receiver, UnboundedSender as Sender};
use futures::io::AsyncWrite;
use futures::lock::Mutex;
use futures::stream::{StreamExt, TryStreamExt};
use hypercore_protocol::schema::*;
use hypercore_protocol::{discovery_key, ProtocolBuilder};
use hypercore_protocol::{Channel, ChannelHandler, StreamContext, StreamHandler};
use log::*;
use pretty_hash::fmt as pretty_fmt;
use std::collections::HashMap;
use std::io;
use std::io::Error;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use wasm_bindgen::prelude::*;

struct Writer(Sender<Vec<u8>>);
impl AsyncWrite for Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        // debug!("SEND: {} {:?}", buf.len(), &buf);
        self.0.unbounded_send(buf.to_vec()).unwrap();
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }
}

pub async fn open(
    recv: Receiver<Vec<u8>>,
    send: Sender<Vec<u8>>,
    key: impl ToString,
) -> std::result::Result<JsValue, JsValue> {
    let is_initiator = true;
    let key = key.to_string();
    info!(
        "init hypercore protocol, is_initiator {} key {}",
        is_initiator, key
    );
    let mut feedstore = FeedStore::new();
    feedstore.add(Feed::new(hex::decode(key).unwrap()));
    let feedstore = Arc::new(feedstore);

    let recv = recv.map(|x| Ok(x));
    let recv = Box::pin(recv);
    let reader = recv.into_async_read();
    let writer = Writer(send);

    let mut protocol = ProtocolBuilder::new(is_initiator)
        .set_handlers(feedstore)
        .from_rw(reader, writer);

    let result = protocol.listen().await;
    debug!("RESULT: {:?}", result);
    Ok(JsValue::null())
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
    ) -> io::Result<()> {
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
    state: Mutex<FeedState>,
}
impl Feed {
    pub fn new(key: Vec<u8>) -> Self {
        Feed {
            discovery_key: discovery_key(&key),
            key,
            state: Mutex::new(FeedState::default()),
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
    async fn on_open<'a>(&self, channel: &mut Channel<'a>, discovery_key: &[u8]) -> io::Result<()> {
        log::info!("open channel {}", pretty_fmt(&discovery_key).unwrap());
        let msg = Want {
            start: 0,
            length: None,
        };
        channel.want(msg).await
    }

    async fn on_have<'a>(&self, channel: &mut Channel<'a>, msg: Have) -> io::Result<()> {
        let mut state = self.state.lock().await;
        // Check if the remote announces a new head.
        log::info!("receive have: {} (state {})", msg.start, state.remote_head);
        if state.remote_head == 0 || msg.start > state.remote_head {
            // Store the new remote head.
            state.remote_head = msg.start;
            // If we didn't start reading, request first data block.
            if !state.started {
                state.started = true;
                let msg = Request {
                    index: 0,
                    bytes: None,
                    hash: None,
                    nodes: None,
                };
                channel.request(msg).await?;
            }
        }
        Ok(())
    }

    async fn on_data<'a>(&self, channel: &mut Channel<'a>, msg: Data) -> io::Result<()> {
        let state = self.state.lock().await;
        log::info!(
            "receive data: idx {}, {} bytes (remote_head {})",
            msg.index,
            msg.value.as_ref().map_or(0, |v| v.len()),
            state.remote_head
        );

        if let Some(value) = msg.value {
            debug!("receive data: {:?}", String::from_utf8(value));
        }

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
