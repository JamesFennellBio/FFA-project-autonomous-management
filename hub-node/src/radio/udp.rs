//! 2.4 GHz path: nodes on Wi-Fi send each frame as one UDP datagram.

use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::warn;

use super::{Inbound, MAX_RADIO_FRAME};
use crate::hub::Link;

pub async fn recv_loop(sock: Arc<UdpSocket>, tx: mpsc::Sender<Inbound>) {
    let mut buf = [0u8; 512];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, addr)) => {
                if n == 0 || n > MAX_RADIO_FRAME {
                    continue;
                }
                if tx
                    .send(Inbound {
                        frame: buf[..n].to_vec(),
                        link: Link::Udp(addr),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(e) => warn!("udp recv: {e}"),
        }
    }
}
