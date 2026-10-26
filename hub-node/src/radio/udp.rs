//! 2.4 GHz path: nodes on Wi-Fi send each frame as one UDP datagram. The
//! phone app's port uses the same loop with bigger frames.

use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::warn;

use super::Inbound;
use crate::hub::Link;

/// Datagrams longer than `max_len` are dropped. `link` says which port it was.
pub async fn recv_loop(
    sock: Arc<UdpSocket>,
    tx: mpsc::Sender<Inbound>,
    max_len: usize,
    link: fn(std::net::SocketAddr) -> Link,
) {
    let mut buf = vec![0u8; max_len + 1];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, addr)) => {
                if n == 0 || n > max_len {
                    continue;
                }
                if tx
                    .send(Inbound {
                        frame: buf[..n].to_vec(),
                        link: link(addr),
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
