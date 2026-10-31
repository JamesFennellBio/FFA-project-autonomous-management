//! End-to-end tests for the v2 hybrid protocol (feature `pq`): pairing with
//! and without QR tickets, the hybrid handshake, AND-composition of the
//! signatures, tampering, downgrade and replay.
//! CRYPTO-REVIEW: these tests are the spec for the v2 security properties.
#![cfg(feature = "pq")]

use crypto_sharecrate::Error;
use crypto_sharecrate::crypto::identity::Identity;
use crypto_sharecrate::crypto::responder::PeerSessions;
use crypto_sharecrate::crypto::session::{MAX_MSGS, V2_MAX_MSGS};
use crypto_sharecrate::crypto::v2::handshake::{self, ACK2_LEN, HELLO2_LEN, HELLO2_SIGNED};
use crypto_sharecrate::crypto::v2::identity::{
    self, ED25519_SIGNATURE_LEN, HybridIdentity, SIGNATURE_LEN,
};
use crypto_sharecrate::crypto::v2::link::{LinkCore, LinkEvent};
use crypto_sharecrate::crypto::v2::pairing::{self, Ticket};
use crypto_sharecrate::crypto::{handshake as v1_handshake, pairing as v1_pairing};
use crypto_sharecrate::frame::FrameType;

const PHONE_SEED: [u8; 32] = [0x31; 32];
const HUB_SEED: [u8; 32] = [0x42; 32];
const TOKEN: [u8; 16] = [0x5A; 16];

fn rand112(tag: u8) -> [u8; 112] {
    let mut r = [0u8; 112];
    for (i, b) in r.iter_mut().enumerate() {
        *b = tag.wrapping_mul(29).wrapping_add(i as u8).wrapping_mul(13) ^ (i as u8).rotate_left(3);
    }
    r
}

struct Hub {
    id: HybridIdentity,
    peers: PeerSessions,
    phone_pub: [u8; identity::PUBLIC_KEY_LEN],
}

/// Pairs a phone using a QR ticket and returns both sides.
fn paired() -> (LinkCore, Hub) {
    let hub_id = HybridIdentity::from_seed(&HUB_SEED);
    let hub_key_id = hub_id.key_id();
    let mut phone = LinkCore::new(&PHONE_SEED, None).with_expected_peer(hub_key_id);
    let req = phone.pair_request(Some(Ticket {
        token: &TOKEN,
        responder_key_id: &hub_key_id,
    }));
    let parsed = pairing::parse_pair_request(&req).unwrap();
    assert_eq!(&parsed.device_pub, phone.public_key());
    pairing::verify_ticket(&parsed, &TOKEN, &hub_key_id).unwrap();

    let acc = pairing::pair_accept(&hub_id, &parsed.device_pub, 9);
    match phone.handle_frame(&acc, 0).unwrap() {
        LinkEvent::Paired(p) => {
            assert_eq!(p.node_id, 9);
            assert_eq!(&p.peer_pub, hub_id.public_key());
        }
        e => panic!("expected Paired, got {e:?}"),
    }
    (
        phone,
        Hub {
            id: hub_id,
            peers: PeerSessions::new(),
            phone_pub: parsed.device_pub,
        },
    )
}

/// Runs one handshake and confirms it with a first frame, as the real links do.
fn connect(phone: &mut LinkCore, hub: &mut Hub, tag: u8, now: u64) {
    let hello = phone.start_handshake(&rand112(tag), now).unwrap();
    assert_eq!(hello.len(), HELLO2_LEN);
    let ack = hub
        .peers
        .on_hello_v2(
            &hello,
            &hub.phone_pub,
            &hub.id,
            &rand112(tag ^ 0xFF),
            1000 + tag as u32,
            now,
        )
        .unwrap();
    assert_eq!(ack.len(), ACK2_LEN);
    assert!(matches!(
        phone.handle_frame(&ack, now).unwrap(),
        LinkEvent::SessionUp { .. }
    ));
    let msg = br#"{"id":1,"req":{"type":"info"}}"#;
    let mut frame = [0u8; 256];
    let n = phone
        .session_mut()
        .unwrap()
        .seal(FrameType::Uplink, msg, &mut frame, now)
        .unwrap();
    let mut out = [0u8; 256];
    let (_, pt) = hub.peers.open(&frame[..n], &mut out, now).unwrap();
    assert_eq!(pt, msg);
}

#[test]
fn hybrid_sign_verify() {
    let id = HybridIdentity::from_seed(&[7u8; 32]);
    let sig = id.sign(b"FFA-TEST", &[b"hello", b"world"]);
    assert_eq!(sig.len(), SIGNATURE_LEN);
    identity::verify(id.public_key(), b"FFA-TEST", &[b"hello", b"world"], &sig).unwrap();
    // Wrong label, wrong message, or wrong key: rejected.
    assert_eq!(
        identity::verify(id.public_key(), b"FFA-OTHER", &[b"hello", b"world"], &sig),
        Err(Error::BadSignature)
    );
    assert_eq!(
        identity::verify(id.public_key(), b"FFA-TEST", &[b"hello", b"w0rld"], &sig),
        Err(Error::BadSignature)
    );
    let other = HybridIdentity::from_seed(&[8u8; 32]);
    assert_eq!(
        identity::verify(other.public_key(), b"FFA-TEST", &[b"hello", b"world"], &sig),
        Err(Error::BadSignature)
    );
}

/// CRYPTO-REVIEW: AND-composition. Breaking either half breaks the signature.
#[test]
fn either_signature_half_failing_rejects() {
    let id = HybridIdentity::from_seed(&[7u8; 32]);
    let sig = id.sign(b"FFA-TEST", &[b"msg"]);

    let mut bad_ed = sig;
    bad_ed[10] ^= 1;
    assert_eq!(
        identity::verify(id.public_key(), b"FFA-TEST", &[b"msg"], &bad_ed),
        Err(Error::BadSignature)
    );

    let mut bad_ml = sig;
    bad_ml[ED25519_SIGNATURE_LEN + 100] ^= 1;
    assert_eq!(
        identity::verify(id.public_key(), b"FFA-TEST", &[b"msg"], &bad_ml),
        Err(Error::BadSignature)
    );

    // Mixing halves from two different keys doesn't verify under either key.
    let other = HybridIdentity::from_seed(&[8u8; 32]);
    let other_sig = other.sign(b"FFA-TEST", &[b"msg"]);
    let mut spliced = sig;
    spliced[ED25519_SIGNATURE_LEN..].copy_from_slice(&other_sig[ED25519_SIGNATURE_LEN..]);
    assert_eq!(
        identity::verify(id.public_key(), b"FFA-TEST", &[b"msg"], &spliced),
        Err(Error::BadSignature)
    );

    // A public key whose ML-DSA half was swapped for another key's fails too.
    let mut swapped = *id.public_key();
    swapped[32..].copy_from_slice(&other.public_key()[32..]);
    assert_eq!(
        identity::verify(&swapped, b"FFA-TEST", &[b"msg"], &sig),
        Err(Error::BadSignature)
    );
    assert_ne!(identity::key_id(&swapped), id.key_id());
}

#[test]
fn key_id_and_fingerprint() {
    let id = HybridIdentity::from_seed(&[7u8; 32]);
    assert_eq!(id.key_id(), identity::key_id(id.public_key()));
    let fp = id.fingerprint_str();
    let fp = core::str::from_utf8(&fp).unwrap();
    assert_eq!(fp.len(), 19);
    assert_eq!(fp.matches('-').count(), 3);
    // The same seed gives a different key from v1, so v1 and v2 never share a key.
    assert_ne!(
        &Identity::from_seed(&[7u8; 32]).public_key()[..],
        &id.public_key()[..32]
    );
}

#[test]
fn full_flow_pair_handshake_traffic() {
    let (mut phone, mut hub) = paired();
    assert!(phone.needs_handshake(0));
    connect(&mut phone, &mut hub, 1, 0);
    assert!(!phone.needs_handshake(1));

    // Reply direction, hub -> phone.
    let reply = br#"{"id":1,"ok":{"name":"hub"}}"#;
    let mut frame = [0u8; 256];
    let n = hub
        .peers
        .current_mut()
        .unwrap()
        .seal(FrameType::Uplink, reply, &mut frame, 1)
        .unwrap();
    let mut out = [0u8; 256];
    let (_, pt) = phone
        .session_mut()
        .unwrap()
        .open(&frame[..n], &mut out, 1)
        .unwrap();
    assert_eq!(pt, reply);

    // A rekey replaces the session and still works.
    connect(&mut phone, &mut hub, 2, 10);
    assert_eq!(phone.sessions_started(), 2);
}

#[test]
fn large_uplink_payload() {
    let (mut phone, mut hub) = paired();
    connect(&mut phone, &mut hub, 3, 0);
    let msg = [0x41u8; 8000];
    let mut frame = [0u8; 8100];
    let n = phone
        .session_mut()
        .unwrap()
        .seal(FrameType::Uplink, &msg, &mut frame, 0)
        .unwrap();
    let mut out = [0u8; 8100];
    let (_, pt) = hub.peers.open(&frame[..n], &mut out, 0).unwrap();
    assert_eq!(pt, &msg[..]);
}

#[test]
fn v2_sessions_use_v2_message_limit() {
    let (mut phone, mut hub) = paired();
    connect(&mut phone, &mut hub, 4, 0);
    let s = phone.session_mut().unwrap();
    let mut frame = [0u8; 64];
    // connect() already sent 1 frame.
    for _ in 1..V2_MAX_MSGS {
        s.seal(FrameType::Uplink, b"x", &mut frame, 0).unwrap();
    }
    assert_eq!(
        s.seal(FrameType::Uplink, b"x", &mut frame, 0),
        Err(Error::RekeyRequired)
    );
    assert!(phone.needs_handshake(0));
    // v1 limit unchanged.
    assert_eq!(MAX_MSGS, 10);
}

#[test]
fn tampered_hello_rejected() {
    let (mut phone, hub) = paired();
    let hello = phone.start_handshake(&rand112(5), 0).unwrap();
    // Flip a byte in the X25519 key, the ML-KEM key, and the nonce.
    for pos in [5usize, 600, HELLO2_SIGNED - 3] {
        let mut h = hello;
        h[pos] ^= 0x80;
        let r = handshake::respond(&h, &hub.phone_pub, &hub.id, 1, &rand112(6), 0);
        assert!(matches!(r, Err(Error::BadSignature)), "byte {pos}");
    }
    // Signed by someone else.
    let evil = HybridIdentity::from_seed(&[0x66; 32]);
    let (_, evil_hello) = handshake::initiate(&evil, 9, &rand112(7));
    assert!(matches!(
        handshake::respond(&evil_hello, &hub.phone_pub, &hub.id, 1, &rand112(6), 0),
        Err(Error::BadSignature)
    ));
}

#[test]
fn tampered_ack_rejected_and_handshake_stays_pending() {
    let (mut phone, mut hub) = paired();
    let hello = phone.start_handshake(&rand112(8), 0).unwrap();
    let ack = hub
        .peers
        .on_hello_v2(&hello, &hub.phone_pub, &hub.id, &rand112(9), 77, 0)
        .unwrap();
    // Flip a byte in the ML-KEM ciphertext.
    let mut bad = ack;
    bad[500] ^= 1;
    assert_eq!(phone.handle_frame(&bad, 0), Err(Error::BadSignature));
    assert!(phone.handshake_pending(0));
    // The genuine ACK still completes the handshake afterwards.
    assert!(matches!(
        phone.handle_frame(&ack, 0).unwrap(),
        LinkEvent::SessionUp { .. }
    ));
}

#[test]
fn ack_from_wrong_responder_rejected() {
    let (mut phone, hub) = paired();
    let hello = phone.start_handshake(&rand112(10), 0).unwrap();
    let evil = HybridIdentity::from_seed(&[0x66; 32]);
    let (ack, _) = handshake::respond(&hello, &hub.phone_pub, &evil, 1, &rand112(11), 0).unwrap();
    assert_eq!(phone.handle_frame(&ack, 0), Err(Error::BadSignature));
}

/// CRYPTO-REVIEW: no downgrade in either direction.
#[test]
fn v1_and_v2_frames_do_not_mix() {
    let (mut phone, hub) = paired();
    let v2_hello = phone.start_handshake(&rand112(12), 0).unwrap();

    // A v2 HELLO is not a v1 HELLO.
    let v1_hub = Identity::from_seed(&HUB_SEED);
    let v1_node = Identity::from_seed(&PHONE_SEED);
    assert!(matches!(
        v1_handshake::respond(&v2_hello, &v1_node.public_key(), &v1_hub, 1, &[1u8; 48], 0),
        Err(Error::BadFrame)
    ));

    // A v1 HELLO is not a v2 HELLO, and v1 handshake/pairing frames are refused on a v2 link.
    let (_, v1_hello) = v1_handshake::initiate(&v1_node, 9, &[2u8; 48]);
    assert!(matches!(
        handshake::respond(&v1_hello, &hub.phone_pub, &hub.id, 1, &rand112(13), 0),
        Err(Error::BadFrame)
    ));
    let v1_accept = v1_pairing::pair_accept(&v1_hub, &v1_node.public_key(), 9);
    assert_eq!(phone.handle_frame(&v1_accept, 0), Err(Error::BadFrame));
    let mut fake_v1_ack = [0u8; 128];
    fake_v1_ack[0] = FrameType::HelloAck as u8;
    assert_eq!(phone.handle_frame(&fake_v1_ack, 0), Err(Error::BadFrame));
}

#[test]
fn replayed_hello_rejected() {
    let (mut phone, mut hub) = paired();
    let hello = phone.start_handshake(&rand112(14), 0).unwrap();
    hub.peers
        .on_hello_v2(&hello, &hub.phone_pub, &hub.id, &rand112(15), 1, 0)
        .unwrap();
    assert_eq!(
        hub.peers
            .on_hello_v2(&hello, &hub.phone_pub, &hub.id, &rand112(16), 2, 0),
        Err(Error::ReplayedHello)
    );
}

#[test]
fn pending_session_needs_key_confirmation() {
    let (mut phone, mut hub) = paired();
    connect(&mut phone, &mut hub, 17, 0);
    let first_epoch = hub.peers.current().unwrap().epoch;
    // A new HELLO makes a pending session; the current one keeps working.
    let hello = phone.start_handshake(&rand112(18), 1).unwrap();
    hub.peers
        .on_hello_v2(&hello, &hub.phone_pub, &hub.id, &rand112(19), 500, 1)
        .unwrap();
    assert_eq!(hub.peers.current().unwrap().epoch, first_epoch);
}

/// CRYPTO-REVIEW: a phone that pinned the hub's KeyId from the QR code
/// refuses a validly signed accept from any other responder.
#[test]
fn pinned_key_id_mismatch_is_wrong_peer() {
    let hub = HybridIdentity::from_seed(&HUB_SEED);
    let evil = HybridIdentity::from_seed(&[0x66; 32]);
    let mut phone = LinkCore::new(&PHONE_SEED, None).with_expected_peer(hub.key_id());
    let acc = pairing::pair_accept(&evil, phone.public_key(), 3);
    assert_eq!(phone.handle_frame(&acc, 0), Err(Error::WrongPeer));
    assert!(!phone.is_paired());
}

#[test]
fn accept_for_another_device_rejected() {
    let hub = HybridIdentity::from_seed(&HUB_SEED);
    let other = HybridIdentity::from_seed(&[0x77; 32]);
    let mut phone = LinkCore::new(&PHONE_SEED, None);
    let acc = pairing::pair_accept(&hub, other.public_key(), 3);
    assert_eq!(phone.handle_frame(&acc, 0), Err(Error::BadSignature));
}

#[test]
fn ticket_checks() {
    let hub = HybridIdentity::from_seed(&HUB_SEED);
    let hub_kid = hub.key_id();
    let phone = HybridIdentity::from_seed(&PHONE_SEED);
    let req = pairing::pair_request(
        &phone,
        Some(Ticket {
            token: &TOKEN,
            responder_key_id: &hub_kid,
        }),
    );
    let parsed = pairing::parse_pair_request(&req).unwrap();
    pairing::verify_ticket(&parsed, &TOKEN, &hub_kid).unwrap();

    // Wrong token.
    assert_eq!(
        pairing::verify_ticket(&parsed, &[0u8; 16], &hub_kid),
        Err(Error::BadTicket)
    );
    // Relayed to a different hub.
    let other_hub = HybridIdentity::from_seed(&[0x88; 32]).key_id();
    assert_eq!(
        pairing::verify_ticket(&parsed, &TOKEN, &other_hub),
        Err(Error::BadTicket)
    );
    // No ticket claimed.
    let plain = pairing::parse_pair_request(&pairing::pair_request(&phone, None)).unwrap();
    assert!(plain.ticket_mac.is_none());
    assert_eq!(
        pairing::verify_ticket(&plain, &TOKEN, &hub_kid),
        Err(Error::BadTicket)
    );

    // The ticket MAC is signed, so it can't be lifted onto another device's request.
    let thief = HybridIdentity::from_seed(&[0x99; 32]);
    let mut stolen = pairing::pair_request(&thief, None);
    stolen[1985] = 1;
    stolen[1986..2018].copy_from_slice(&req[1986..2018]);
    assert_eq!(
        pairing::parse_pair_request(&stolen).err(),
        Some(Error::BadSignature)
    );
}

#[test]
fn saved_pairing_for_other_key_dropped_when_pinned() {
    let (phone, _hub) = paired();
    let saved = phone.pairing().unwrap();
    let other = HybridIdentity::from_seed(&[0x55; 32]).key_id();
    let re = LinkCore::new(&PHONE_SEED, Some(saved)).with_expected_peer(other);
    assert!(!re.is_paired());
}
