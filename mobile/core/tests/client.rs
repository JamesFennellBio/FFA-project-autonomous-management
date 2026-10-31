//! HubClient against a small stand-in hub that speaks the real protocol
//! (crypto-sharecrate v2 pairing, PeerSessions, Uplink frames over TCP). The
//! real hub is covered by hub-node's tests and the end-to-end script.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use crypto_sharecrate::crypto::responder::PeerSessions;
use crypto_sharecrate::crypto::session::V2_MAX_MSGS;
use crypto_sharecrate::crypto::v2::identity::{HybridIdentity, HybridPublic};
use crypto_sharecrate::crypto::v2::pairing;
use crypto_sharecrate::frame::FrameType;
use farmlab_mobile_core::{HubClient, MobileError, new_identity_seed, parse_pairing_qr};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use uplink::{read_frame, write_frame};

const HUB_SEED: [u8; 32] = [1u8; 32];
const PHONE_ID: u16 = 7;
/// The QR ticket token the stand-in hub has open.
const TICKET: [u8; 16] = [0x5A; 16];

/// Accepts the phone on its second pairing request (as if someone clicked
/// Approve in between), or on the first one if it proves it scanned the QR
/// code for [`TICKET`] (a request with any other ticket is ignored). Then it echoes every request. With `drop_first`, the
/// first request is ignored so the client has to retry. HELLO2s whose number
/// (1-based) is in `drop_hellos` get no reply. State is shared across
/// connections, like the real hub's.
async fn mini_hub(drop_first: bool) -> SocketAddr {
    mini_hub_with(drop_first, &[]).await
}

struct HubState {
    identity: HybridIdentity,
    start: Instant,
    pair_requests: usize,
    phone: Option<Box<HybridPublic>>,
    sessions: PeerSessions,
    dropped: bool,
    hellos: usize,
}

async fn mini_hub_with(drop_first: bool, drop_hellos: &'static [usize]) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(tokio::sync::Mutex::new(HubState {
        identity: HybridIdentity::from_seed(&HUB_SEED),
        start: Instant::now(),
        pair_requests: 0,
        phone: None,
        sessions: PeerSessions::new(),
        dropped: !drop_first,
        hellos: 0,
    }));
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let state = state.clone();
            tokio::spawn(async move {
                while let Ok(f) = read_frame(&mut sock).await {
                    let mut st = state.lock().await;
                    let now = st.start.elapsed().as_secs();
                    let reply = match FrameType::from_byte(f[0]).unwrap() {
                        FrameType::PairRequest2 => {
                            let req = pairing::parse_pair_request(&f).unwrap();
                            st.pair_requests += 1;
                            if req.ticket_mac.is_some() {
                                let kid = st.identity.key_id();
                                if pairing::verify_ticket(&req, &TICKET, &kid).is_err() {
                                    continue;
                                }
                            } else if st.pair_requests < 2 {
                                continue;
                            }
                            let acc = pairing::pair_accept(&st.identity, &req.device_pub, PHONE_ID);
                            st.phone = Some(Box::new(req.device_pub));
                            acc.to_vec()
                        }
                        FrameType::Hello2 => {
                            st.hellos += 1;
                            if drop_hellos.contains(&st.hellos) {
                                continue;
                            }
                            let rand = uplink::v2::hs_random().unwrap();
                            let st = &mut *st;
                            st.sessions
                                .on_hello_v2(
                                    &f,
                                    st.phone.as_ref().unwrap(),
                                    &st.identity,
                                    &rand,
                                    now as u32,
                                    now,
                                )
                                .unwrap()
                                .to_vec()
                        }
                        FrameType::Uplink => {
                            let st = &mut *st;
                            let v: Value = uplink::open_json(&f, |f, out| {
                                st.sessions.open(f, out, now).map(|(_, pt)| pt.len())
                            })
                            .unwrap();
                            if !st.dropped {
                                st.dropped = true;
                                continue;
                            }
                            let body = if v["req"]["type"] == "fail" {
                                json!({ "id": v["id"], "err": "nope" })
                            } else {
                                json!({ "id": v["id"], "ok": { "echo": v["req"] } })
                            };
                            uplink::seal_json(st.sessions.current_mut().unwrap(), &body, now)
                                .unwrap()
                        }
                        _ => continue,
                    };
                    drop(st);
                    if write_frame(&mut sock, &reply).await.is_err() {
                        break;
                    }
                }
            });
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
        uplink::v2::public_to_hex(HybridIdentity::from_seed(&HUB_SEED).public_key())
    );
    // More requests than one session allows: a rekey on the way.
    for i in 0..(V2_MAX_MSGS as i64 + 5) {
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
    let someone_else = hex::encode(HybridIdentity::from_seed(&[5u8; 32]).key_id());
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
    // HELLO2 1 starts the first session; HELLO2s 2-4 (the rekey after 1000
    // requests, tried 3 times) go unanswered; HELLO2 5 works again.
    let addr = mini_hub_with(false, &[2, 3, 4]).await;
    let c = client(addr, None);
    c.pair(15).await.unwrap();
    for _ in 0..V2_MAX_MSGS {
        c.request(r#"{"type":"info"}"#.into()).await.unwrap();
    }
    let r = c.request(r#"{"type":"info"}"#.into()).await;
    assert!(matches!(r, Err(MobileError::Timeout)), "{r:?}");
    // Straight away, while the last HELLO is still "pending": must handshake
    // again rather than try the used-up session.
    let r = c.request(r#"{"type":"info"}"#.into()).await;
    assert!(r.is_ok(), "{r:?}");
}

fn qr_for(addr: SocketAddr, token: [u8; 16]) -> String {
    uplink::pairlink::PhoneQr {
        addr: addr.to_string(),
        hub_key_id: HybridIdentity::from_seed(&HUB_SEED).key_id(),
        token,
        expires: 1_900_000_000,
    }
    .to_uri()
}

#[tokio::test]
async fn a_qr_code_pairs_at_once_with_the_pinned_hub() {
    let addr = mini_hub(false).await;
    let uri = qr_for(addr, TICKET);
    let qr = parse_pairing_qr(uri.clone()).unwrap();
    assert_eq!(qr.hub_addr, addr.to_string());
    assert_eq!(
        qr.hub_fingerprint,
        farmlab_mobile_core::fingerprint_of_key(qr.hub_key_id.clone()).unwrap()
    );
    let c = client(addr, Some(qr.hub_key_id));
    let t = Instant::now();
    let p = c.pair_with_qr(uri, 10).await.unwrap();
    // First request accepted: no approval round (that takes a 3 s retry).
    assert!(t.elapsed().as_secs() < 2, "{:?}", t.elapsed());
    assert_eq!(p.phone_id, PHONE_ID);
    let v: Value =
        serde_json::from_str(&c.request(r#"{"type":"info"}"#.into()).await.unwrap()).unwrap();
    assert_eq!(v["echo"]["type"], "info");
}

#[tokio::test]
async fn a_wrong_qr_ticket_is_not_accepted() {
    let addr = mini_hub(false).await;
    let uri = qr_for(addr, [1u8; 16]);
    let qr = parse_pairing_qr(uri.clone()).unwrap();
    let c = client(addr, Some(qr.hub_key_id));
    assert!(matches!(
        c.pair_with_qr(uri, 4).await,
        Err(MobileError::Pairing(_))
    ));
}

#[tokio::test]
async fn qr_pairing_needs_a_client_made_from_that_qr_code() {
    let addr = mini_hub(false).await;
    let uri = qr_for(addr, TICKET);
    // Not pinned to the QR code's hub.
    assert!(matches!(
        client(addr, None).pair_with_qr(uri.clone(), 4).await,
        Err(MobileError::Invalid(_))
    ));
    // A different hub address.
    let qr = parse_pairing_qr(uri.clone()).unwrap();
    let other = HubClient::new(
        new_identity_seed().unwrap(),
        "127.0.0.1:1".into(),
        None,
        Some(qr.hub_key_id),
    )
    .unwrap();
    assert!(matches!(
        other.pair_with_qr(uri, 4).await,
        Err(MobileError::Invalid(_))
    ));
    assert!(parse_pairing_qr("https://example.com".into()).is_err());
}
