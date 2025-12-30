//! WebSocket support for real-time event streaming
//!
//! Provides WebSocket connections for pushing events to clients:
//! - New blocks
//! - New transactions
//! - Mempool updates
//! - Peer status changes
//! - Minting status

use futures::{SinkExt, StreamExt};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, error, info, warn};

/// Events that can be pushed to WebSocket clients
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum WsEvent {
    #[serde(rename = "block")]
    NewBlock {
        height: u64,
        hash: String,
        timestamp: u64,
        tx_count: usize,
        difficulty: u64,
    },
    #[serde(rename = "transaction")]
    NewTransaction {
        hash: String,
        fee: u64,
        /// Block height if confirmed, None if in mempool
        in_block: Option<u64>,
    },
    #[serde(rename = "mempool")]
    MempoolUpdate {
        size: usize,
        total_fees: u64,
    },
    #[serde(rename = "peers")]
    PeerStatus {
        peer_count: usize,
        event: PeerEvent,
    },
    #[serde(rename = "minting")]
    MintingStatus {
        active: bool,
        hashrate: f64,
        blocks_found: u64,
    },
}

/// Peer connection events
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerEvent {
    Connected { peer_id: String },
    Disconnected { peer_id: String },
    CountChanged,
}

/// Event types clients can subscribe to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Blocks,
    Transactions,
    Mempool,
    Peers,
    Minting,
}

impl WsEvent {
    /// Get the event type for filtering
    pub fn event_type(&self) -> EventType {
        match self {
            WsEvent::NewBlock { .. } => EventType::Blocks,
            WsEvent::NewTransaction { .. } => EventType::Transactions,
            WsEvent::MempoolUpdate { .. } => EventType::Mempool,
            WsEvent::PeerStatus { .. } => EventType::Peers,
            WsEvent::MintingStatus { .. } => EventType::Minting,
        }
    }
}

/// Client subscription preferences
#[derive(Debug, Default)]
struct WsSubscription {
    events: HashSet<EventType>,
}

impl WsSubscription {
    fn new() -> Self {
        Self {
            events: HashSet::new(),
        }
    }

    fn subscribe(&mut self, event: EventType) {
        self.events.insert(event);
    }

    fn unsubscribe(&mut self, event: EventType) {
        self.events.remove(&event);
    }

    fn subscribe_all(&mut self) {
        self.events.insert(EventType::Blocks);
        self.events.insert(EventType::Transactions);
        self.events.insert(EventType::Mempool);
        self.events.insert(EventType::Peers);
        self.events.insert(EventType::Minting);
    }

    fn is_subscribed(&self, event_type: EventType) -> bool {
        self.events.contains(&event_type)
    }
}

/// Incoming client messages
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientMessage {
    #[serde(rename = "subscribe")]
    Subscribe { events: Vec<EventType> },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { events: Vec<EventType> },
    #[serde(rename = "ping")]
    Ping,
}

/// Outgoing server messages
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ServerMessage<'a> {
    #[serde(rename = "event")]
    Event {
        #[serde(flatten)]
        event: &'a WsEvent,
    },
    #[serde(rename = "subscribed")]
    Subscribed { events: Vec<&'static str> },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "error")]
    Error { message: String },
}

/// Broadcaster for WebSocket events
///
/// Clone this to share across the application. Events sent to the broadcaster
/// are delivered to all connected WebSocket clients (filtered by subscription).
#[derive(Clone)]
pub struct WsBroadcaster {
    sender: broadcast::Sender<WsEvent>,
}

impl WsBroadcaster {
    /// Create a new broadcaster with the given channel capacity
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Create a new receiver for events
    pub fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.sender.subscribe()
    }

    /// Send an event to all connected clients
    pub fn send(&self, event: WsEvent) {
        // Ignore send errors - they just mean no receivers are connected
        let _ = self.sender.send(event);
    }

    /// Send a new block event
    pub fn new_block(&self, height: u64, hash: &[u8], timestamp: u64, tx_count: usize, difficulty: u64) {
        self.send(WsEvent::NewBlock {
            height,
            hash: hex::encode(hash),
            timestamp,
            tx_count,
            difficulty,
        });
    }

    /// Send a new transaction event
    pub fn new_transaction(&self, hash: &[u8], fee: u64, in_block: Option<u64>) {
        self.send(WsEvent::NewTransaction {
            hash: hex::encode(hash),
            fee,
            in_block,
        });
    }

    /// Send a mempool update event
    pub fn mempool_update(&self, size: usize, total_fees: u64) {
        self.send(WsEvent::MempoolUpdate { size, total_fees });
    }

    /// Send a peer status event
    pub fn peer_connected(&self, peer_count: usize, peer_id: &str) {
        self.send(WsEvent::PeerStatus {
            peer_count,
            event: PeerEvent::Connected {
                peer_id: peer_id.to_string(),
            },
        });
    }

    /// Send a peer disconnected event
    pub fn peer_disconnected(&self, peer_count: usize, peer_id: &str) {
        self.send(WsEvent::PeerStatus {
            peer_count,
            event: PeerEvent::Disconnected {
                peer_id: peer_id.to_string(),
            },
        });
    }

    /// Send a peer count changed event
    pub fn peer_count_changed(&self, peer_count: usize) {
        self.send(WsEvent::PeerStatus {
            peer_count,
            event: PeerEvent::CountChanged,
        });
    }

    /// Send a minting status event
    pub fn minting_status(&self, active: bool, hashrate: f64, blocks_found: u64) {
        self.send(WsEvent::MintingStatus {
            active,
            hashrate,
            blocks_found,
        });
    }
}

/// Handle a WebSocket connection
pub async fn handle_websocket(upgraded: Upgraded, broadcaster: Arc<WsBroadcaster>) {
    let ws_stream = WebSocketStream::from_raw_socket(
        TokioIo::new(upgraded),
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        None,
    )
    .await;

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let mut subscription = WsSubscription::new();
    let mut event_receiver = broadcaster.subscribe();

    info!("WebSocket client connected");

    loop {
        tokio::select! {
            // Handle incoming messages from client
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Subscribe { events }) => {
                                for event in events {
                                    subscription.subscribe(event);
                                }
                                let subscribed: Vec<&'static str> = subscription.events
                                    .iter()
                                    .map(|e| match e {
                                        EventType::Blocks => "blocks",
                                        EventType::Transactions => "transactions",
                                        EventType::Mempool => "mempool",
                                        EventType::Peers => "peers",
                                        EventType::Minting => "minting",
                                    })
                                    .collect();
                                let response = ServerMessage::Subscribed { events: subscribed };
                                if let Err(e) = ws_sender.send(Message::Text(serde_json::to_string(&response).unwrap().into())).await {
                                    error!("Failed to send subscription confirmation: {}", e);
                                    break;
                                }
                            }
                            Ok(ClientMessage::Unsubscribe { events }) => {
                                for event in events {
                                    subscription.unsubscribe(event);
                                }
                            }
                            Ok(ClientMessage::Ping) => {
                                let response = ServerMessage::Pong;
                                if let Err(e) = ws_sender.send(Message::Text(serde_json::to_string(&response).unwrap().into())).await {
                                    error!("Failed to send pong: {}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!("Invalid client message: {}", e);
                                let response = ServerMessage::Error { message: format!("Invalid message: {}", e) };
                                if let Err(e) = ws_sender.send(Message::Text(serde_json::to_string(&response).unwrap().into())).await {
                                    error!("Failed to send error: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if let Err(e) = ws_sender.send(Message::Pong(data)).await {
                            error!("Failed to send pong: {}", e);
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        debug!("WebSocket client requested close");
                        break;
                    }
                    Some(Ok(_)) => {
                        // Ignore other message types (Binary, Pong, Frame)
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        debug!("WebSocket stream ended");
                        break;
                    }
                }
            }

            // Handle events from broadcaster
            event = event_receiver.recv() => {
                match event {
                    Ok(event) => {
                        // Only send if client is subscribed to this event type
                        if subscription.is_subscribed(event.event_type()) {
                            let message = ServerMessage::Event { event: &event };
                            if let Ok(json) = serde_json::to_string(&message) {
                                if let Err(e) = ws_sender.send(Message::Text(json.into())).await {
                                    error!("Failed to send event: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WebSocket client lagged, missed {} events", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Event broadcaster closed");
                        break;
                    }
                }
            }
        }
    }

    info!("WebSocket client disconnected");
}
