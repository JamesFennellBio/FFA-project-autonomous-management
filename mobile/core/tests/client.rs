//! HubClient against a small stand-in hub that speaks the real protocol
//! (crypto-sharecrate pairing, PeerSessions, Uplink frames). The real hub is
//! covered by hub-node's tests and the end-to-end script.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use crypto_sharecrate::crypto::identity::Identity;
use crypto_sharecrate::crypto::{pairing, responder::PeerSessions};
use crypto_sharecrate::frame::FrameType;
use ffa_mobile_core::{HubClient, MobileError, new_identity_seed};
use serde_json::{Value, json};
use tokio::net::UdpSocket;

const HUB_SEED: [u8; 32] = [1u8; 32];
const PHONE_ID: u16 = 7;

/// Accepts the phone on its second pairing request (as if someone clicked
/// Approve in between), then echoes every request. With `drop_first`, the
/// first request is ignored so the client has to retry. HELLOs whose number
/// (1-based) is in `drop_hellos` get no reply.
async fn mini_hub(drop_first: bool) -> SocketAddr {
    mini_hub_with(drop_first, &[]).await
}

async fn mini_hub_with(drop_first: bool, drop_hellos: &'static [usize]) -> SocketAddr {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let identity = Identity::from_seed(&HUB_SEED);
        let start = Instant::now();
        let mut pair_requests = 0;
        let mut phone: Option<[u8; 32]> = None;
        let mut sessions = PeerSessions::new();
        let mut dropped = !drop_first;
        let mut hellos = 0;
        let mut buf = vec![0u8; 2048];
        loop {
            let (n, from) = sock.recv_from(&mut buf).await.unwrap();
            let f = &buf[..n];
            let now = start.elapsed().as_secs();
            let reply = match FrameType::from_byte(f[0]).unwrap() {
                FrameType::PairRequest => {
                    let pk = pairing::parse_pair_request(f).unwrap();
                    pair_requests += 1;
                    if pair_requests < 2 {
                        continue;
                    }
                    phone = Some(pk);
                    pairing::pair_accept(&identity, &pk, PHONE_ID).to_vec()
                }
                FrameType::Hello => {
                    hellos += 1;
                    if drop_hellos.contains(&hellos) {
                        continue;
                    }
                    let mut rand = [0u8; 48];
                    uplink::getrandom(&mut rand).unwrap();
                    sessions
                        .on_hello(f, &phone.unwrap(), &identity, &rand, now as u32, now)
                        .unwrap()
                        .to_vec()
                }
                FrameType::Uplink => {
                    let v: Value = uplink::open_json(f, |f, out| {
                        sessions.open(f, out, now).map(|(_, pt)| pt.len())
                    })
                    .unwrap();
                    if !dropped {
                        dropped = true;
                        continue;
                    }
                    let body = if v["req"]["type"] == "fail" {
                        json!({ "id": v["id"], "err": "nope" })
                    } else {
                        json!({ "id": v["id"], "ok": { "echo": v["req"] } })
                    };
                    uplink::seal_json(sessions.current_mut().unwrap(), &body, now).unwrap()
                }
                _ => continue,
            };
            sock.send_to(&reply, from).await.unwrap();
        }
    });
    addr
}

fn client(addr: SocketAddr, expected: Option<String>) -> Arc<HubClient> {
    HubClient::new(
        new_identity_seed().unwrap(),
        addr.to_string(),
        None,
        expected,
    )
    .unwrap()
}

#[tokio::test]
async fn pairs_then_keeps_working_across_rekeys() {
    let addr = mini_hub(false).await;
    let c = client(addr, None);
    assert!(matches!(
        c.request(r#"{"type":"info"}"#.into()).await,
        Err(MobileError::NotPaired)
    ));
    let p = c.pair(15).await.unwrap();
    assert_eq!(p.phone_id, PHONE_ID);
    assert_eq!(
        p.hub_public_key,
        uplink::key_to_hex(&Identity::from_seed(&HUB_SEED).public_key())
    );
    // 25 requests = 50 messages: several handshakes on the way.
    for i in 0..25 {
        let r = c
            .request(json!({ "type": "info", "n": i }).to_string())
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["echo"]["n"], i);
    }
}

#[tokio::test]
async fn a_lost_reply_is_retried() {
    let addr = mini_hub(true).await;
    let c = client(addr, None);
    c.pair(15).await.unwrap();
    let r = c.request(r#"{"type":"info"}"#.into()).await.unwrap();
    assert!(r.contains("echo"));
}

#[tokio::test]
async fn hub_refusals_come_back_as_hub_errors() {
    let addr = mini_hub(false).await;
    let c = client(addr, None);
    c.pair(15).await.unwrap();
    match c.request(r#"{"type":"fail"}"#.into()).await {
        Err(MobileError::Hub(m)) => assert_eq!(m, "nope"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_pinned_hub_key_refuses_any_other_hub() {
    let addr = mini_hub(false).await;
    let someone_else = uplink::key_to_hex(&Identity::from_seed(&[5u8; 32]).public_key());
    let c = client(addr, Some(someone_else));
    assert!(matches!(c.pair(15).await, Err(MobileError::Pairing(_))));
    assert!(c.pairing().await.is_none(), "nothing pinned");
}

#[tokio::test]
async fn a_saved_pairing_is_used_without_pairing_again() {
    let addr = mini_hub(false).await;
    let seed = new_identity_seed().unwrap();
    let first = HubClient::new(seed.clone(), addr.to_string(), None, None).unwrap();
    let p = first.pair(15).await.unwrap();
    // e.g. the app restarted: same seed, saved pairing
    let again = HubClient::new(seed, addr.to_string(), Some(p.clone()), None).unwrap();
    assert_eq!(again.pairing().await, Some(p));
    assert!(again.request(r#"{"type":"info"}"#.into()).await.is_ok());
}

#[tokio::test]
async fn recovers_after_a_rekey_times_out() {
    // HELLO 1 starts the first session; HELLOs 2-4 (the rekey after 10
    // requests, tried 3 times) go unanswered; HELLO 5 works again.
    let addr = mini_hub_with(false, &[2, 3, 4]).await;
    let c = client(addr, None);
    c.pair(15).await.unwrap();
    for _ in 0..10 {
        c.request(r#"{"type":"info"}"#.into()).await.unwrap();
    }
    assert!(matches!(
        c.request(r#"{"type":"info"}"#.into()).await,
        Err(MobileError::Timeout)
    ));
    // Straight away, while the last HELLO is still "pending": must handshake
    // again rather than try the used-up session.
    let r = c.request(r#"{"type":"info"}"#.into()).await;
    assert!(r.is_ok(), "{r:?}");
}
