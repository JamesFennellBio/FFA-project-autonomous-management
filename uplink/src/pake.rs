//! CRYPTO-REVIEW: first hub <-> PC pairing with a one-time code (SPAKE2).
//!
//! `hub-node pair` opens a pairing window on TCP [`PAIR_PORT`] and prints a
//! 12-digit code. The person types it into the desktop app, which makes the
//! PC daemon connect and run:
//!
//! ```text
//! PC  -> hub   0x20 | SPAKE2 msg (B, "farmlab-pc")
//! hub -> PC    0x21 | SPAKE2 msg (A, "farmlab-hub")
//!              K = SPAKE2 key (password = code, both ids, both messages)
//!              pc_mac, hub_mac = HKDF-SHA256(K, salt = SHA-256(label || msg_hub || msg_pc))
//! PC  -> hub   0x22 | pc_pub(1984) | hub_listen_port(2) | hub_id(2) | HMAC(pc_mac, ...)
//! hub -> PC    0x23 | hub_pub(1984) | HMAC(hub_mac, ... || PC payload || hub payload)
//!   (or 0x24 | attempts left, when the PC's MAC is wrong)
//! ```
//!
//! Properties this aims for (please check):
//! - **Right Pi:** only the hub that printed the code can produce `hub_mac`,
//!   and only someone who read the code can produce `pc_mac`. Each side pins
//!   the other's hybrid key only after the other's MAC checks out.
//! - **No offline guessing:** SPAKE2 gives an attacker one guess per run. The
//!   hub allows three wrong tries, then closes the window. The code has 10^12
//!   values, so the chance of guessing it is about 3 in 10^12.
//! - **Single use:** the window closes after one success. The code is never saved.
//! - **The PC proves itself first**, so a wrong code learns nothing about the
//!   hub's key. The hub's MAC also covers the PC's payload (both sides agree
//!   on what was exchanged).
//! - The hybrid keys exchanged here are then proven by the normal HELLO2
//!   signatures (possession), so a MAC'd but stolen public key is useless.

use anyhow::{Result, bail};
use crypto_sharecrate::crypto::v2::identity::{HybridIdentity, HybridPublic, PUBLIC_KEY_LEN};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity as SpakeId, Password, Spake2};
use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::Zeroize;

use crate::{read_frame, write_frame};

/// TCP port the hub listens on while a pairing window is open.
pub const PAIR_PORT: u16 = 7402;
/// Digits in a pairing code (shown as three groups of four).
pub const CODE_DIGITS: usize = 12;
/// Wrong codes allowed before the hub closes the window.
pub const MAX_ATTEMPTS: u32 = 3;

const T_PAKE_PC: u8 = 0x20;
const T_PAKE_HUB: u8 = 0x21;
const T_CONFIRM_PC: u8 = 0x22;
const T_CONFIRM_HUB: u8 = 0x23;
const T_WRONG_CODE: u8 = 0x24;
const MAC_LEN: usize = 32;
const PC_PAYLOAD_LEN: usize = PUBLIC_KEY_LEN + 4;

const ID_HUB: &[u8] = b"farmlab-hub";
const ID_PC: &[u8] = b"farmlab-pc";

/// A new random code, formatted `1234-5678-9012`.
///
/// CRYPTO-REVIEW: uniform over 10^12 values (rejection sampling on OS randomness).
pub fn new_code() -> Result<String> {
    const LIMIT: u64 = 1_000_000_000_000;
    // Largest multiple of LIMIT below 2^64, so every code is equally likely.
    const ZONE: u64 = u64::MAX - (u64::MAX % LIMIT);
    loop {
        let mut b = [0u8; 8];
        crate::getrandom(&mut b)?;
        let v = u64::from_le_bytes(b);
        if v < ZONE {
            let d = format!("{:012}", v % LIMIT);
            return Ok(format!("{}-{}-{}", &d[..4], &d[4..8], &d[8..]));
        }
    }
}

/// Digits only, as typed or scanned. Rejects anything that isn't 12 digits.
pub fn normalize_code(s: &str) -> Result<String> {
    let d: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if d.len() != CODE_DIGITS || !d.bytes().all(|b| b.is_ascii_digit()) {
        bail!("the pairing code is 12 digits, like 4821-7730-1956");
    }
    Ok(d)
}

fn password(code: &str) -> Result<Password> {
    Ok(Password::new(
        format!("FFA-PAIR-CODE-v1:{}", normalize_code(code)?).as_bytes(),
    ))
}

struct Macs {
    pc: [u8; 32],
    hub: [u8; 32],
}

impl Drop for Macs {
    fn drop(&mut self) {
        self.pc.zeroize();
        self.hub.zeroize();
    }
}

/// CRYPTO-REVIEW: confirmation keys from the SPAKE2 key, bound to both messages.
fn macs(mut k: Vec<u8>, msg_hub: &[u8], msg_pc: &[u8]) -> Macs {
    let salt: [u8; 32] = Sha256::new()
        .chain_update(b"FFA-PAKE-v1")
        .chain_update(msg_hub)
        .chain_update(msg_pc)
        .finalize()
        .into();
    let hk = Hkdf::<Sha256>::new(Some(&salt), &k);
    k.zeroize();
    let mut m = Macs {
        pc: [0u8; 32],
        hub: [0u8; 32],
    };
    hk.expand(b"FFA-PAKE-v1 pc confirm", &mut m.pc)
        .expect("hkdf length");
    hk.expand(b"FFA-PAKE-v1 hub confirm", &mut m.hub)
        .expect("hkdf length");
    m
}

fn mac(key: &[u8; 32], label: &[u8], parts: &[&[u8]]) -> Hmac<Sha256> {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
    m.update(label);
    for p in parts {
        m.update(&(p.len() as u32).to_le_bytes());
        m.update(p);
    }
    m
}

/// What the PC learns: the hub's key to pin.
pub struct HubPaired {
    pub hub_pub: Box<HybridPublic>,
}

/// What the hub learns: the PC's key, where to reach it, and its own id there.
pub struct PcPaired {
    pub pc_pub: Box<HybridPublic>,
    pub hub_listen_port: u16,
    pub hub_id: u16,
}

/// Why a pairing run failed.
#[derive(Debug)]
pub enum PakeError {
    /// The other side used a different code. `attempts_left` is from the hub.
    WrongCode {
        attempts_left: Option<u32>,
    },
    Other(anyhow::Error),
}

impl std::fmt::Display for PakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PakeError::WrongCode {
                attempts_left: Some(n),
            } => write!(f, "wrong pairing code ({n} tries left)"),
            PakeError::WrongCode {
                attempts_left: None,
            } => write!(f, "wrong pairing code"),
            PakeError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for PakeError {}

impl From<anyhow::Error> for PakeError {
    fn from(e: anyhow::Error) -> Self {
        PakeError::Other(e)
    }
}

async fn expect<S: AsyncRead + Unpin>(s: &mut S, t: u8) -> Result<Vec<u8>, PakeError> {
    let f = read_frame(s).await?;
    if f[0] == T_WRONG_CODE {
        return Err(PakeError::WrongCode {
            attempts_left: f.get(1).map(|n| u32::from(*n)),
        });
    }
    if f[0] != t {
        return Err(anyhow::anyhow!("unexpected pairing message {:#x}", f[0]).into());
    }
    Ok(f[1..].to_vec())
}

fn framed(t: u8, parts: &[&[u8]]) -> Vec<u8> {
    let mut v = vec![t];
    for p in parts {
        v.extend_from_slice(p);
    }
    v
}

/// PC side: run the exchange on a connection to the hub's pairing port.
pub async fn pc_side<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    code: &str,
    pc: &HybridIdentity,
    hub_listen_port: u16,
    hub_id: u16,
) -> Result<HubPaired, PakeError> {
    let (spake, msg_pc) = Spake2::<Ed25519Group>::start_b(
        &password(code)?,
        &SpakeId::new(ID_HUB),
        &SpakeId::new(ID_PC),
    );
    write_frame(s, &framed(T_PAKE_PC, &[&msg_pc])).await?;
    let msg_hub = expect(s, T_PAKE_HUB).await?;
    let k = spake
        .finish(&msg_hub)
        .map_err(|e| anyhow::anyhow!("spake2: {e:?}"))?;
    let m = macs(k, &msg_hub, &msg_pc);

    let mut payload = Vec::with_capacity(PC_PAYLOAD_LEN);
    payload.extend_from_slice(pc.public_key());
    payload.extend_from_slice(&hub_listen_port.to_le_bytes());
    payload.extend_from_slice(&hub_id.to_le_bytes());
    let tag = mac(&m.pc, b"FFA-PAKE-CONFIRM-PC", &[&payload])
        .finalize()
        .into_bytes();
    write_frame(s, &framed(T_CONFIRM_PC, &[&payload, &tag])).await?;

    let reply = expect(s, T_CONFIRM_HUB).await?;
    if reply.len() != PUBLIC_KEY_LEN + MAC_LEN {
        return Err(anyhow::anyhow!("bad hub confirmation").into());
    }
    let (hub_pub, hub_tag) = reply.split_at(PUBLIC_KEY_LEN);
    // CRYPTO-REVIEW: constant-time check that the hub knew the code too.
    mac(&m.hub, b"FFA-PAKE-CONFIRM-HUB", &[&payload, hub_pub])
        .verify_slice(hub_tag)
        .map_err(|_| PakeError::WrongCode {
            attempts_left: None,
        })?;
    let mut k = Box::new([0u8; PUBLIC_KEY_LEN]);
    k.copy_from_slice(hub_pub);
    Ok(HubPaired { hub_pub: k })
}

/// Hub side: one pairing attempt on an accepted connection.
/// `attempts_left` is shown to the PC after a wrong code (not counting this one).
pub async fn hub_side<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    code: &str,
    hub: &HybridIdentity,
    attempts_left: u32,
) -> Result<PcPaired, PakeError> {
    let msg_pc = expect(s, T_PAKE_PC).await?;
    let (spake, msg_hub) = Spake2::<Ed25519Group>::start_a(
        &password(code)?,
        &SpakeId::new(ID_HUB),
        &SpakeId::new(ID_PC),
    );
    write_frame(s, &framed(T_PAKE_HUB, &[&msg_hub])).await?;
    let k = spake
        .finish(&msg_pc)
        .map_err(|e| anyhow::anyhow!("spake2: {e:?}"))?;
    let m = macs(k, &msg_hub, &msg_pc);

    let confirm = expect(s, T_CONFIRM_PC).await?;
    if confirm.len() != PC_PAYLOAD_LEN + MAC_LEN {
        return Err(anyhow::anyhow!("bad PC confirmation").into());
    }
    let (payload, tag) = confirm.split_at(PC_PAYLOAD_LEN);
    // CRYPTO-REVIEW: constant-time check that the PC knew the code. A wrong
    // code is all an attacker learns; the hub's key is only sent after this.
    if mac(&m.pc, b"FFA-PAKE-CONFIRM-PC", &[payload])
        .verify_slice(tag)
        .is_err()
    {
        let _ = write_frame(s, &[T_WRONG_CODE, attempts_left.min(255) as u8]).await;
        return Err(PakeError::WrongCode {
            attempts_left: Some(attempts_left),
        });
    }
    let hub_tag = mac(
        &m.hub,
        b"FFA-PAKE-CONFIRM-HUB",
        &[payload, hub.public_key()],
    )
    .finalize()
    .into_bytes();
    write_frame(s, &framed(T_CONFIRM_HUB, &[hub.public_key(), &hub_tag])).await?;

    let mut pc_pub = Box::new([0u8; PUBLIC_KEY_LEN]);
    pc_pub.copy_from_slice(&payload[..PUBLIC_KEY_LEN]);
    Ok(PcPaired {
        pc_pub,
        hub_listen_port: u16::from_le_bytes([payload[PUBLIC_KEY_LEN], payload[PUBLIC_KEY_LEN + 1]]),
        hub_id: u16::from_le_bytes([payload[PUBLIC_KEY_LEN + 2], payload[PUBLIC_KEY_LEN + 3]]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (HybridIdentity, HybridIdentity) {
        (
            HybridIdentity::from_seed(&[1u8; 32]),
            HybridIdentity::from_seed(&[2u8; 32]),
        )
    }

    #[test]
    fn codes() {
        let c = new_code().unwrap();
        assert_eq!(c.len(), 14);
        assert_eq!(normalize_code(&c).unwrap().len(), 12);
        assert_eq!(normalize_code(" 4821 7730-1956 ").unwrap(), "482177301956");
        assert!(normalize_code("4821-7730-195").is_err());
        assert!(normalize_code("4821-7730-195a").is_err());
    }

    #[tokio::test]
    async fn right_code_pairs_both_ways() {
        let (hub, pc) = ids();
        let (mut a, mut b) = tokio::io::duplex(16 * 1024);
        let code = new_code().unwrap();
        let c2 = code.clone();
        let h = tokio::spawn(async move { hub_side(&mut a, &c2, &hub, 2).await.map(|p| (p, hub)) });
        let got = pc_side(&mut b, &code, &pc, 7400, 1).await.unwrap();
        let (p, hub) = h.await.unwrap().unwrap();
        assert_eq!(&*got.hub_pub, hub.public_key());
        assert_eq!(&*p.pc_pub, pc.public_key());
        assert_eq!((p.hub_listen_port, p.hub_id), (7400, 1));
    }

    #[tokio::test]
    async fn wrong_code_fails_on_both_sides_and_reveals_no_hub_key() {
        let (hub, pc) = ids();
        let (mut a, mut b) = tokio::io::duplex(16 * 1024);
        let h = tokio::spawn(async move { hub_side(&mut a, "1111-1111-1111", &hub, 2).await });
        let r = pc_side(&mut b, "1111-1111-1112", &pc, 7400, 1).await;
        assert!(matches!(
            r,
            Err(PakeError::WrongCode {
                attempts_left: Some(2)
            })
        ));
        assert!(matches!(h.await.unwrap(), Err(PakeError::WrongCode { .. })));
    }
}
