//! End-to-end protocol tests: pairing, handshake, rekey policy, tampering, replay.
//! CRYPTO-REVIEW: these tests are the spec for the security properties.

use crypto_sharecrate::Error;
use crypto_sharecrate::crypto::identity::Identity;
use crypto_sharecrate::crypto::pairing;
use crypto_sharecrate::crypto::responder::PeerSessions;
use crypto_sharecrate::crypto::session::{MAX_AGE_SECS, MAX_MSGS};
use crypto_sharecrate::node::{NodeCore, NodeEvent, Pairing};
use crypto_sharecrate::packet::{Packet, Reading};

const NODE_SEED: [u8; 32] = [0x11; 32];
const HUB_SEED: [u8; 32] = [0x22; 32];

fn rand48(tag: u8) -> [u8; 48] {
    let mut r = [0u8; 48];
    for (i, b) in r.iter_mut().enumerate() {
        *b = tag.wrapping_mul(31).wrapping_add(i as u8).wrapping_mul(7) | 1;
    }
    r
}

struct Hub {
    id: Identity,
    peer: PeerSessions,
    node_pub: [u8; 32],
}

/// Runs pairing and returns a paired node plus hub state.
fn paired() -> (NodeCore, Hub) {
    let hub_id = Identity::from_seed(&HUB_SEED);
    let mut node = NodeCore::new(&NODE_SEED, None);
    let req = node.pair_request();
    let node_pub = pairing::parse_pair_request(&req).unwrap();
    assert_eq!(node_pub, node.public_key());
    let acc = pairing::pair_accept(&hub_id, &node_pub, 5);
    match node.handle_frame(&acc, 0).unwrap() {
        NodeEvent::Paired(p) => {
            assert_eq!(p.node_id, 5);
            assert_eq!(p.peer_pub, hub_id.public_key());
        }
        e => panic!("{e:?}"),
    }
    (
        node,
        Hub {
            id: hub_id,
            peer: PeerSessions::new(),
            node_pub,
        },
    )
}

fn handshake(node: &mut NodeCore, hub: &mut Hub, now: u64, tag: u8) {
    assert!(node.needs_handshake(now));
    let hello = node.start_handshake(&rand48(tag), now).unwrap();
    let ack = hub
        .peer
        .on_hello(
            &hello,
            &hub.node_pub,
            &hub.id,
            &rand48(tag ^ 0x80),
            1000 + tag as u32,
            now,
        )
        .unwrap();
    assert!(matches!(
        node.handle_frame(&ack, now).unwrap(),
        NodeEvent::SessionUp { .. }
    ));
}

fn data(v: i32) -> Packet {
    Packet::data(&[Reading {
        channel: 0,
        value: v,
    }])
}

fn hub_open(hub: &mut Hub, frame: &[u8], now: u64) -> Result<Packet, Error> {
    let mut buf = [0u8; 64];
    let (h, pt) = hub.peer.open(frame, &mut buf, now)?;
    Packet::decode(h.ftype, pt)
}

#[test]
fn full_roundtrip_both_directions() {
    let (mut node, mut hub) = paired();
    handshake(&mut node, &mut hub, 0, 1);

    let f = node.seal(&data(23_500), 1).unwrap();
    assert_eq!(f.len(), 64);
    assert_eq!(hub_open(&mut hub, &f, 1).unwrap(), data(23_500));

    // hub -> node command (the session was promoted by the frame above)
    let cmd = hub
        .peer
        .current_mut()
        .unwrap()
        .seal_packet(
            &Packet::Command {
                channel: 3,
                value: 1,
            },
            2,
        )
        .unwrap();
    assert_eq!(
        node.handle_frame(&cmd, 2).unwrap(),
        NodeEvent::Command {
            channel: 3,
            value: 1
        }
    );
}

#[test]
fn rekey_after_ten_messages() {
    let (mut node, mut hub) = paired();
    handshake(&mut node, &mut hub, 0, 1);
    for i in 0..MAX_MSGS {
        assert!(!node.needs_handshake(1));
        let f = node.seal(&data(i as i32), 1).unwrap();
        hub_open(&mut hub, &f, 1).unwrap();
    }
    // 11th send is refused, the node has to rekey
    assert!(node.needs_handshake(1));
    assert_eq!(node.seal(&data(99), 1), Err(Error::RekeyRequired));
    let old_epoch = node.session().unwrap().epoch;
    handshake(&mut node, &mut hub, 1, 2);
    assert_ne!(node.session().unwrap().epoch, old_epoch);
    let f = node.seal(&data(100), 1).unwrap();
    assert_eq!(hub_open(&mut hub, &f, 1).unwrap(), data(100));
    assert_eq!(node.sessions_started(), 2);
}

#[test]
fn rekey_after_one_hour() {
    let (mut node, mut hub) = paired();
    handshake(&mut node, &mut hub, 0, 1);
    node.seal(&data(1), 10).unwrap();
    assert!(!node.needs_handshake(MAX_AGE_SECS - 1));
    assert!(node.needs_handshake(MAX_AGE_SECS));
    assert_eq!(node.seal(&data(1), MAX_AGE_SECS), Err(Error::RekeyRequired));
}

#[test]
fn hub_rejects_expired_session() {
    let (mut node, mut hub) = paired();
    handshake(&mut node, &mut hub, 0, 1);
    let f = node.seal(&data(1), 0).unwrap();
    assert_eq!(
        hub_open(&mut hub, &f, MAX_AGE_SECS + 120),
        Err(Error::RekeyRequired)
    );
}

#[test]
fn tampered_ciphertext_header_and_tag_rejected() {
    let (mut node, mut hub) = paired();
    handshake(&mut node, &mut hub, 0, 1);
    let good = node.seal(&data(5), 0).unwrap();
    for idx in [0usize, 2, 5, 9, 20, 63] {
        let mut bad = good;
        bad[idx] ^= 0x01;
        assert!(
            hub_open(&mut hub, &bad, 0).is_err(),
            "flip at {idx} accepted"
        );
    }
    // the untouched frame still works: failed frames didn't move the replay window
    assert!(hub_open(&mut hub, &good, 0).is_ok());
}

#[test]
fn replay_rejected() {
    let (mut node, mut hub) = paired();
    handshake(&mut node, &mut hub, 0, 1);
    let f1 = node.seal(&data(1), 0).unwrap();
    let f2 = node.seal(&data(2), 0).unwrap();
    hub_open(&mut hub, &f2, 0).unwrap();
    assert_eq!(hub_open(&mut hub, &f2, 0), Err(Error::Replay));
    assert_eq!(hub_open(&mut hub, &f1, 0), Err(Error::Replay)); // older counter
}

#[test]
fn hello_signed_by_wrong_key_rejected() {
    let (_, mut hub) = paired();
    let impostor = NodeCore::new(
        &[0x33; 32],
        Some(Pairing {
            node_id: 5,
            peer_pub: hub.id.public_key(),
        }),
    );
    let mut impostor = impostor;
    let hello = impostor.start_handshake(&rand48(9), 0).unwrap();
    let r = hub
        .peer
        .on_hello(&hello, &hub.node_pub, &hub.id, &rand48(1), 7, 0);
    assert_eq!(r.err(), Some(Error::BadSignature));
}

#[test]
fn ack_from_wrong_hub_rejected() {
    let (mut node, hub) = paired();
    let fake_hub = Identity::from_seed(&[0x44; 32]);
    let mut fake = PeerSessions::new();
    let hello = node.start_handshake(&rand48(1), 0).unwrap();
    let ack = fake
        .on_hello(&hello, &hub.node_pub, &fake_hub, &rand48(2), 7, 0)
        .unwrap();
    assert_eq!(node.handle_frame(&ack, 0), Err(Error::BadSignature));
}

#[test]
fn replayed_hello_rejected_and_live_session_survives() {
    let (mut node, mut hub) = paired();
    let hello = node.start_handshake(&rand48(1), 0).unwrap();
    let ack = hub
        .peer
        .on_hello(&hello, &hub.node_pub, &hub.id, &rand48(2), 7, 0)
        .unwrap();
    node.handle_frame(&ack, 0).unwrap();
    hub_open(&mut hub, &node.seal(&data(1), 0).unwrap(), 0).unwrap();

    // attacker replays the old HELLO: refused outright
    assert_eq!(
        hub.peer
            .on_hello(&hello, &hub.node_pub, &hub.id, &rand48(3), 8, 0)
            .err(),
        Some(Error::ReplayedHello)
    );
    // the live session keeps working
    assert_eq!(
        hub_open(&mut hub, &node.seal(&data(2), 0).unwrap(), 0).unwrap(),
        data(2)
    );
}

#[test]
fn old_replay_cannot_evict_genuine_pending_session() {
    let (mut node, mut hub) = paired();
    // attacker recorded an earlier HELLO from another boot of this node
    let mut old = NodeCore::new(&NODE_SEED, node.pairing());
    let recorded = old.start_handshake(&rand48(40), 0).unwrap();

    // genuine handshake starts...
    let hello = node.start_handshake(&rand48(1), 0).unwrap();
    let ack = hub
        .peer
        .on_hello(&hello, &hub.node_pub, &hub.id, &rand48(2), 7, 0)
        .unwrap();
    // ...the attacker replays the recorded HELLO before the node's first frame
    hub.peer
        .on_hello(&recorded, &hub.node_pub, &hub.id, &rand48(3), 8, 0)
        .unwrap();
    // the genuine session still confirms
    node.handle_frame(&ack, 0).unwrap();
    assert_eq!(
        hub_open(&mut hub, &node.seal(&data(5), 0).unwrap(), 0).unwrap(),
        data(5)
    );
}

#[test]
fn pinned_hub_key_rejects_other_hubs() {
    let hub_id = Identity::from_seed(&HUB_SEED);
    let attacker = Identity::from_seed(&[0x66; 32]);
    let mut node = NodeCore::new(&NODE_SEED, None).with_expected_peer(hub_id.public_key());
    let pk = node.public_key();
    // attacker answers first with a validly signed accept for this node
    let evil = pairing::pair_accept(&attacker, &pk, 3);
    assert_eq!(node.handle_frame(&evil, 0), Err(Error::WrongPeer));
    assert!(!node.is_paired());
    // the real hub's accept still works
    let good = pairing::pair_accept(&hub_id, &pk, 3);
    assert!(matches!(
        node.handle_frame(&good, 0),
        Ok(NodeEvent::Paired(_))
    ));
}

#[test]
fn saved_pairing_with_wrong_key_is_dropped_when_pinned() {
    let other = Identity::from_seed(&[0x77; 32]);
    let node = NodeCore::new(
        &NODE_SEED,
        Some(Pairing {
            node_id: 1,
            peer_pub: other.public_key(),
        }),
    )
    .with_expected_peer(Identity::from_seed(&HUB_SEED).public_key());
    assert!(!node.is_paired());
}

#[test]
fn pair_accept_for_other_device_rejected() {
    let hub_id = Identity::from_seed(&HUB_SEED);
    let other = Identity::from_seed(&[0x55; 32]);
    let mut node = NodeCore::new(&NODE_SEED, None);
    let acc = pairing::pair_accept(&hub_id, &other.public_key(), 9);
    assert_eq!(node.handle_frame(&acc, 0), Err(Error::BadSignature));
    assert!(!node.is_paired());
}

#[test]
fn forged_pair_request_rejected() {
    let node = NodeCore::new(&NODE_SEED, None);
    let mut req = node.pair_request();
    req[1] ^= 1; // swap in a different public key
    assert!(pairing::parse_pair_request(&req).is_err());
}
