use futures::channel::mpsc::UnboundedSender as Sender;
use futures::lock::Mutex;
use futures::sink::SinkExt;
use futures::stream::StreamExt;
use hypercore_protocol::schema::*;
use hypercore_protocol::{discovery_key, Channel, Event, Message, Protocol};
use log::*;
use pretty_hash::fmt as pretty_fmt;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::Arc;
use wasm_bindgen_futures::spawn_local;

use crate::ws::{ReadHalf, WriteHalf};
use crate::AppEvent;

pub async fn replicate(
    mut protocol: Protocol<ReadHalf, WriteHalf>,
    key: impl AsRef<str>,
    app_tx: Sender<AppEvent>,
) -> io::Result<()> {
    let mut feedstore = FeedStore::new();
    feedstore.add(Feed::new(hex::decode(key.as_ref().as_bytes()).unwrap()));
    let feedstore = Arc::new(feedstore);

    while let Some(event) = protocol.next().await {
        let event = event.expect("Protocol error");
        match event {
            Event::Handshake(_remote_public_key) => {
                debug!("received handshake from remote");
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
                    let mut app_tx = app_tx.clone();
                    feed.on_open(&mut channel).await.unwrap();
                    spawn_local(async move {
                        while let Some(message) = channel.next().await {
                            feed.on_message(&mut channel, message, &mut app_tx)
                                .await
                                .unwrap();
                        }
                    });
                }
            }
            _ => {}
        }
    }
    Ok(())
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
        app_tx: &mut Sender<AppEvent>,
    ) -> io::Result<()> {
        // debug!("receive message: {:?}", message);
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
        app_tx: &mut Sender<AppEvent>,
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
    pub data: BTreeMap<u64, Vec<u8>>,
}
impl Default for FeedState {
    fn default() -> Self {
        FeedState {
            remote_head: 0,
            started: false,
            data: BTreeMap::new(),
        }
    }
}
