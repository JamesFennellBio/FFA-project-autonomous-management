//! `hub-node pair`: pair this hub with the PC using a one-time code.
//!
//! It opens a pairing window on `pair_listen` (TCP 7402) for
//! [`WINDOW`], prints the code and a terminal QR, and waits for the desktop
//! app (through pc-daemon) to run the SPAKE2 exchange in `uplink::pake`. On
//! success it writes pc_pairing.json; the running hub service picks that up
//! within a few seconds. Three wrong codes close the window.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crypto_sharecrate::crypto::v2::identity::{self, HybridIdentity, KeyId};
use tokio::net::TcpListener;
use tokio::time::Instant;
use uplink::pake::{self, MAX_ATTEMPTS, PakeError, PcPaired};

/// How long the pairing window stays open.
pub const WINDOW: Duration = Duration::from_secs(600);
/// One pairing run must finish within this (a stalled connection can't hold the window).
const RUN_TIMEOUT: Duration = Duration::from_secs(20);

/// A successful pairing: the PC's key and where it listens for the hub.
pub struct Paired {
    pub pc: PcPaired,
    pub pc_addr: SocketAddr,
}

/// Accepts connections until one PC proves it has `code`, too many wrong
/// codes arrive, or `deadline` passes.
///
/// CRYPTO-REVIEW: one success closes the window (the code is single use);
/// [`MAX_ATTEMPTS`] wrong codes close it too. A run that fails for any other
/// reason (garbage, timeout) doesn't use up an attempt, since it never tested
/// a code. With `pin`, a PC with a different key is refused.
pub async fn window(
    listener: &TcpListener,
    code: &str,
    hub: &HybridIdentity,
    deadline: Instant,
    pin: Option<KeyId>,
) -> Result<Paired> {
    let mut wrong = 0;
    loop {
        let (mut stream, peer) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .context("the pairing window closed (10 minutes); run `hub-node pair` again")??;
        let left = MAX_ATTEMPTS - wrong - 1;
        let run = tokio::time::timeout(RUN_TIMEOUT, pake::hub_side(&mut stream, code, hub, left));
        match run.await {
            Ok(Ok(pc)) => {
                if let Some(k) = pin
                    && identity::key_id(&pc.pc_pub) != k
                {
                    bail!(
                        "the PC that knew the code is not the one pinned by pc_key_id in hub.toml"
                    );
                }
                let pc_addr = SocketAddr::new(peer.ip(), pc.hub_listen_port);
                return Ok(Paired { pc, pc_addr });
            }
            Ok(Err(PakeError::WrongCode { .. })) => {
                wrong += 1;
                eprintln!(
                    "Wrong code entered from {} ({wrong} of {MAX_ATTEMPTS}).",
                    peer.ip()
                );
                if wrong >= MAX_ATTEMPTS {
                    bail!(
                        "{MAX_ATTEMPTS} wrong codes: the pairing window is closed. Run `hub-node pair` again."
                    );
                }
            }
            Ok(Err(e)) => eprintln!("Pairing attempt from {} failed: {e}", peer.ip()),
            Err(_) => eprintln!("Pairing attempt from {} timed out.", peer.ip()),
        }
    }
}

/// This machine's LAN address (the one used to reach other hosts), for the QR code.
fn lan_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // connect() on UDP sends nothing; it only picks the outgoing interface.
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

pub async fn run(
    data_dir: &Path,
    pairing_path: &PathBuf,
    listen: &str,
    pin: Option<KeyId>,
    replace: bool,
) -> Result<()> {
    if pairing_path.exists() && !replace {
        bail!(
            "this hub is already paired with a PC. To pair with a different PC, run `hub-node pair --replace`"
        );
    }
    let seed = crate::seed::load(data_dir)?;
    let hub = HybridIdentity::from_seed(&seed);
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("opening the pairing port {listen}"))?;
    let port = listener.local_addr()?.port();
    let code = pake::new_code()?;
    let ip = lan_ip()
        .map(|i| i.to_string())
        .unwrap_or_else(|| "<hub IP>".into());
    let addr = format!("{ip}:{port}");

    println!("Pairing window open for 10 minutes on {addr}");
    println!(
        "Hub fingerprint  {}",
        uplink::v2::fingerprint(hub.public_key())
    );
    println!();
    println!("  In the desktop app: Pairing > Add hub, enter {ip} and this code:");
    println!();
    println!("      {code}");
    println!();
    println!("  or scan this with the computer's camera:");
    print!(
        "{}",
        uplink::pairlink::qr_terminal(&uplink::pairlink::hub_code_uri(&addr, &code))?
    );
    println!();

    let paired = window(&listener, &code, &hub, Instant::now() + WINDOW, pin).await?;
    crate::pc_link::save_pairing(
        pairing_path,
        paired.pc.hub_id,
        &paired.pc.pc_pub,
        &paired.pc_addr.to_string(),
    )?;
    println!(
        "Paired with PC {} at {} (as hub {}). The hub service connects to it within a few seconds.",
        uplink::v2::fingerprint(&paired.pc.pc_pub),
        paired.pc_addr,
        paired.pc.hub_id
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open() -> (TcpListener, SocketAddr) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = l.local_addr().unwrap();
        (l, a)
    }

    async fn try_code(addr: SocketAddr, code: &str, pc: &HybridIdentity) -> Result<(), PakeError> {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        pake::pc_side(&mut s, code, pc, 7400, 1).await.map(|_| ())
    }

    #[tokio::test]
    async fn right_code_pairs_and_learns_the_pc_address() {
        let (l, addr) = open().await;
        let hub = HybridIdentity::from_seed(&[1u8; 32]);
        let pc = HybridIdentity::from_seed(&[2u8; 32]);
        let code = pake::new_code().unwrap();
        let c = code.clone();
        let w = tokio::spawn(async move {
            window(&l, &c, &hub, Instant::now() + Duration::from_secs(30), None).await
        });
        try_code(addr, &code, &pc).await.unwrap();
        let p = w.await.unwrap().unwrap();
        assert_eq!(&*p.pc.pc_pub, pc.public_key());
        assert_eq!(p.pc_addr, "127.0.0.1:7400".parse().unwrap());
    }

    #[tokio::test]
    async fn three_wrong_codes_close_the_window() {
        let (l, addr) = open().await;
        let hub = HybridIdentity::from_seed(&[1u8; 32]);
        let pc = HybridIdentity::from_seed(&[2u8; 32]);
        let w = tokio::spawn(async move {
            window(
                &l,
                "1234-5678-9012",
                &hub,
                Instant::now() + Duration::from_secs(30),
                None,
            )
            .await
        });
        for left in [2u32, 1, 0] {
            let r = try_code(addr, "0000-0000-0000", &pc).await;
            assert!(matches!(r, Err(PakeError::WrongCode { attempts_left: Some(n) }) if n == left));
        }
        let err = w.await.unwrap().err().unwrap().to_string();
        assert!(err.contains("wrong codes"), "{err}");
        // The window is gone: even the right code can't connect now.
        assert!(tokio::net::TcpStream::connect(addr).await.is_err());
    }

    #[tokio::test]
    async fn a_pinned_pc_key_refuses_other_pcs() {
        let (l, addr) = open().await;
        let hub = HybridIdentity::from_seed(&[1u8; 32]);
        let pc = HybridIdentity::from_seed(&[2u8; 32]);
        let other = HybridIdentity::from_seed(&[3u8; 32]).key_id();
        let w = tokio::spawn(async move {
            window(
                &l,
                "1234-5678-9012",
                &hub,
                Instant::now() + Duration::from_secs(30),
                Some(other),
            )
            .await
        });
        let _ = try_code(addr, "1234-5678-9012", &pc).await;
        assert!(w.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn the_window_times_out() {
        let (l, _) = open().await;
        let hub = HybridIdentity::from_seed(&[1u8; 32]);
        let r = window(
            &l,
            "1234-5678-9012",
            &hub,
            Instant::now() + Duration::from_millis(50),
            None,
        )
        .await;
        assert!(r.err().unwrap().to_string().contains("closed"));
    }
}
