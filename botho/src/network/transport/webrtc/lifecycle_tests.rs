// Copyright (c) 2026 Botho Foundation
//! Real UDP/DTLS/SCTP tests: loopback only, no public STUN, TURN, DNS or
//! signaling.
use super::*;
use std::{net::UdpSocket, sync::Mutex as StdMutex, time::Duration};
use tokio::time::{sleep, timeout};
use webrtc::data_channel::RTCDataChannelState;

fn transport() -> WebRtcTransport {
    WebRtcTransport::new(
        IceConfig {
            stun_servers: vec![],
            turn_servers: vec![],
            gathering_timeout: Duration::from_secs(2),
            ..IceConfig::default()
        },
        StunConfig::default(),
    )
}

async fn until(mut condition: impl AsyncFnMut() -> bool) {
    timeout(Duration::from_secs(10), async {
        while !condition().await {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("local lifecycle operation timed out");
}

async fn pair() -> (WebRtcConnection, WebRtcConnection, SocketAddr, SocketAddr) {
    let transport = transport();
    let left = transport
        .create_peer_connection_on("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let right = transport
        .create_peer_connection_on("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let outbound = WebRtcTransport::create_data_channel(&left, "botho")
        .await
        .unwrap();
    assert!(outbound.ordered().await.unwrap());
    assert_eq!(outbound.max_retransmits().await.unwrap(), None);
    let early = Arc::new(StdMutex::new(Vec::new()));
    let observed = early.clone();
    transport.gatherer().on_candidate(&left, move |candidate| {
        observed.lock().unwrap().push(candidate)
    });
    WebRtcTransport::create_offer(&left).await.unwrap();
    let left_candidates = transport.wait_for_ice_gathering(&left).await.unwrap();
    assert_eq!(left_candidates.len(), early.lock().unwrap().len());
    let late = Arc::new(StdMutex::new(Vec::new()));
    let observed = late.clone();
    transport.gatherer().on_candidate(&left, move |candidate| {
        observed.lock().unwrap().push(candidate)
    });
    assert_eq!(left_candidates.len(), late.lock().unwrap().len());
    assert_eq!(
        transport.wait_for_ice_gathering(&left).await.unwrap().len(),
        left_candidates.len()
    );
    let offer = left.local_description().await.unwrap();
    WebRtcTransport::create_answer(&right, offer).await.unwrap();
    let right_candidates = transport.wait_for_ice_gathering(&right).await.unwrap();
    WebRtcTransport::set_remote_answer(&left, right.local_description().await.unwrap())
        .await
        .unwrap();
    let inbound = timeout(Duration::from_secs(10), right.accept_data_channel())
        .await
        .unwrap()
        .unwrap();
    until(async || {
        outbound.ready_state().await.unwrap() == RTCDataChannelState::Open
            && inbound.ready_state().await.unwrap() == RTCDataChannelState::Open
    })
    .await;
    let left_addr = format!("{}:{}", left_candidates[0].address, left_candidates[0].port)
        .parse()
        .unwrap();
    let right_addr = format!(
        "{}:{}",
        right_candidates[0].address, right_candidates[0].port
    )
    .parse()
    .unwrap();
    assert_eq!(left_candidates[0].address, "127.0.0.1");
    assert_eq!(right_candidates[0].address, "127.0.0.1");
    (
        WebRtcConnection::new(left, outbound),
        WebRtcConnection::new(right, inbound),
        left_addr,
        right_addr,
    )
}

async fn exchange(sender: &WebRtcConnection, receiver: &WebRtcConnection) {
    let payload: Vec<u8> = (0..8192).map(|n| (n % 251) as u8).collect();
    assert_eq!(receiver.recv(&mut [0; 3]).await.unwrap(), 0);
    sender.send(&payload).await.unwrap();
    let mut received = Vec::new();
    timeout(Duration::from_secs(10), async {
        while received.len() < payload.len() {
            let mut chunk = [0; 137];
            let n = receiver.recv(&mut chunk).await.unwrap();
            received.extend_from_slice(&chunk[..n]);
            if n == 0 {
                sleep(Duration::from_millis(5)).await;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(received, payload);
}

async fn released(address: SocketAddr) {
    until(async || UdpSocket::bind(address).is_ok()).await;
}

#[tokio::test]
async fn local_delivery_disconnect_and_fresh_reconnect() {
    for _ in 0..2 {
        let (left, right, left_addr, right_addr) =
            timeout(Duration::from_secs(20), pair()).await.unwrap();
        assert!(left.is_connected());
        assert!(right.is_connected());
        exchange(&left, &right).await;
        exchange(&right, &left).await;
        timeout(Duration::from_secs(2), left.close())
            .await
            .unwrap()
            .unwrap();
        assert!(!left.is_connected());
        assert!(left.send(b"closed").await.is_err());
        // Remote SCTP close must stop its receiver, without another payload.
        until(async || right.recv_buffer.lock().await.closed).await;
        let _ = timeout(Duration::from_secs(2), right.close())
            .await
            .unwrap();
        released(left_addr).await;
        released(right_addr).await;
    }
}

#[tokio::test]
async fn drop_connection_stops_receiver_and_releases_socket() {
    let (left, right, left_addr, right_addr) = pair().await;
    let receiver = left.receiver_abort.clone();
    drop(left);
    until(async || receiver.is_finished()).await;
    released(left_addr).await;
    drop(right);
    released(right_addr).await;
}

#[tokio::test]
async fn failed_handshake_and_close_during_accept_release_socket() {
    let transport = transport();
    assert!(transport
        .create_peer_connection_on("0.0.0.0:0".parse().unwrap())
        .await
        .is_err());
    let peer = transport
        .create_peer_connection_on("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let channel = WebRtcTransport::create_data_channel(&peer, "botho")
        .await
        .unwrap();
    WebRtcTransport::create_offer(&peer).await.unwrap();
    let candidates = transport.wait_for_ice_gathering(&peer).await.unwrap();
    let address = format!("{}:{}", candidates[0].address, candidates[0].port)
        .parse()
        .unwrap();
    // No remote description: the channel cannot open or send.
    assert!(channel
        .send(bytes::BytesMut::from(&b"not connected"[..]))
        .await
        .is_err());
    let waiter_peer = peer.clone();
    let waiter = tokio::spawn(async move { waiter_peer.accept_data_channel().await });
    peer.close().await.unwrap();
    assert!(timeout(Duration::from_secs(2), waiter)
        .await
        .unwrap()
        .unwrap()
        .is_none());
    released(address).await;
    // The same transport can create a new peer after failure.
    let replacement = transport.create_peer_connection_on(address).await.unwrap();
    replacement.close().await.unwrap();
}

#[tokio::test]
async fn receive_overflow_fails_closed_without_unbounded_buffering() {
    let (left, right, _, _) = pair().await;
    // Place the real receiver just below its limit, then deliver an actual SCTP
    // message.
    right
        .recv_buffer
        .lock()
        .await
        .bytes
        .resize(MAX_RECEIVE_BYTES - 1, 0);
    left.send(b"overflow").await.unwrap();
    until(async || right.recv_buffer.lock().await.failed).await;
    assert!(right.recv(&mut [0; 8]).await.is_err());
    assert!(right.recv_buffer.lock().await.bytes.len() <= MAX_RECEIVE_BYTES);
    assert!(right.send(b"closed").await.is_err());
    let _ = left.close().await;
    let _ = right.close().await;
}

#[tokio::test]
async fn close_wakes_pending_gathering_without_waiting_for_its_deadline() {
    let peer = transport()
        .create_peer_connection_on("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert!(
        WebRtcTransport::create_answer(&peer, RTCSessionDescription::default())
            .await
            .is_err()
    );
    let waiting_peer = peer.clone();
    let waiting =
        tokio::spawn(async move { transport().wait_for_ice_gathering(&waiting_peer).await });
    tokio::task::yield_now().await;
    peer.close().await.unwrap();
    let result = timeout(Duration::from_millis(200), waiting)
        .await
        .expect("close must wake ICE waiters")
        .unwrap();
    assert!(matches!(
        result,
        Err(TransportError::Ice(IceError::NoCandidates))
    ));
}

#[tokio::test]
async fn concurrent_close_waits_for_the_first_teardown_and_notifies_remote() {
    let (left, right, left_addr, right_addr) = pair().await;
    let mut first = Box::pin(left.close());
    assert!(futures::poll!(first.as_mut()).is_pending());
    // Leave the first caller suspended after starting teardown. A second caller
    // must not stop its driver while that caller is awaiting the SCTP close event.
    let mut second = Box::pin(left.close());
    assert!(
        timeout(Duration::from_millis(100), second.as_mut())
            .await
            .is_err(),
        "second close bypassed the pending first teardown"
    );
    timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(2), second)
        .await
        .unwrap()
        .unwrap();
    until(async || right.recv_buffer.lock().await.closed).await;
    released(left_addr).await;
    let _ = right.close().await;
    released(right_addr).await;
}

#[tokio::test]
async fn cancelled_close_then_drop_aborts_receiver_and_releases_socket() {
    let (left, right, left_addr, right_addr) = pair().await;
    let receiver = left.receiver_abort.clone();
    let mut closing = Box::pin(left.close());
    assert!(futures::poll!(closing.as_mut()).is_pending());
    assert!(
        !receiver.is_finished(),
        "exercise cancellation before receiver completion"
    );
    drop(closing);
    drop(left);
    until(async || receiver.is_finished()).await;
    released(left_addr).await;
    drop(right);
    released(right_addr).await;
}
