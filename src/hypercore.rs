use futures::channel::mpsc::{UnboundedReceiver as Receiver, UnboundedSender as Sender};
use futures::io::AsyncWrite;
use futures::lock::Mutex;
use futures::sink::SinkExt;
use futures::stream::{StreamExt, TryStreamExt};
use hypercore_protocol::{discovery_key, Channel, Event, ProtocolBuilder};
use hypercore_protocol::{schema::*, Message};
use log::*;
use pretty_hash::fmt as pretty_fmt;
use std::collections::HashMap;
use std::io;
use std::io::Error;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::AppEvent;

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
    app_tx: Sender<AppEvent>,
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

    let mut protocol = ProtocolBuilder::new(is_initiator).connect_rw(reader, writer);

    spawn_local(async move {
        while let Some(event) = protocol.next().await {
            let event = event.expect("Protocol error");
            match event {
                Event::Handshake(_remote_public_key) => {
                    debug!("Handshake from remote");
                }
                Event::DiscoveryKey(discovery_key) => {
                    if let Some(feed) = feedstore.get(&discovery_key) {
                        debug!(
                            "open discovery_key: {}",
                            pretty_fmt(&discovery_key).unwrap()
                        );
                        protocol.open(feed.key.clone()).await.unwrap();
                    } else {
                        debug!(
                            "unknown discovery_key: {}",
                            pretty_fmt(&discovery_key).unwrap()
                        );
                    }
                }
                Event::Channel(mut channel) => {
                    if let Some(feed) = feedstore.get(channel.discovery_key()) {
                        let feed = feed.clone();
                        let app_tx = app_tx.clone();
                        feed.on_open(&mut channel).await.unwrap();
                        spawn_local(async move {
                            while let Some(message) = channel.next().await {
                                feed.on_message(&mut channel, message, app_tx.clone())
                                    .await
                                    .unwrap();
                            }
                        });
                    }
                }
                _ => {}
            }
        }
    });

    // let result = protocol.listen().await;
    // debug!("RESULT: {:?}", result);
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
    async fn on_message(
        &self,
        channel: &mut Channel,
        message: Message,
        app_tx: Sender<AppEvent>,
    ) -> io::Result<()> {
        debug!("receive message: {:?}", message);
        match message {
            Message::Have(message) => self.on_have(channel, message).await,
            Message::Data(message) => self.on_data(channel, message, app_tx).await,
            _ => Ok(()),
        }
    }

    async fn on_open(&self, channel: &mut Channel) -> io::Result<()> {
        log::info!(
            "open channel {}",
            pretty_fmt(channel.discovery_key()).unwrap()
        );
        let msg = Want {
            start: 0,
            length: None,
        };
        channel.want(msg).await
    }

    async fn on_have(&self, channel: &mut Channel, msg: Have) -> io::Result<()> {
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

    async fn on_data(
        &self,
        channel: &mut Channel,
        msg: Data,
        mut app_tx: Sender<AppEvent>,
    ) -> io::Result<()> {
        let mut state = self.state.lock().await;
        log::info!(
            "receive data: idx {}, {} bytes (remote_head {})",
            msg.index,
            msg.value.as_ref().map_or(0, |v| v.len()),
            state.remote_head
        );

        if let Some(value) = msg.value.clone() {
            debug!("receive data: {:?}", String::from_utf8(value.clone()));
            state.data.insert(msg.index, value);
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
        } else {
            let text = state
                .data
                .values()
                .map(|b| String::from_utf8(b.to_vec()).unwrap())
                .collect::<String>();
            app_tx.send(AppEvent::ContentLoaded(text)).await.unwrap();
        }

        Ok(())
    }
}

/// A FeedState stores the head seq of the remote.
/// This would have a bitfield to support sparse sync in the actual impl.
#[derive(Debug)]
struct FeedState {
    pub remote_head: u64,
    pub started: bool,
    pub data: HashMap<u64, Vec<u8>>,
}
impl Default for FeedState {
    fn default() -> Self {
        FeedState {
            remote_head: 0,
            started: false,
            data: HashMap::new(),
        }
    }
}
