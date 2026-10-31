//! TCP sockets for the phone app (`uplink::app::APP_PORT`).
//!
//! Each connection gets an id. Frames it sends go into the hub's main loop as
//! `Link::App(id)`; replies for that id come back through [`Phones::send`] to
//! the connection's writer task. The protocol itself (pairing, HELLO2,
//! requests) is in `hub/app.rs`. This file only moves bytes.
//!
//! Limits, since anyone on the network can connect:
//! - at most [`MAX_CONNECTIONS`] at once; when full, the oldest is closed
//!   (a real phone just reconnects on its next request)
//! - a connection that sends nothing for [`IDLE_TIMEOUT`] is closed
//! - each frame is capped by `uplink::MAX_TCP_FRAME` (`read_frame`), and the
//!   hub checks sizes per frame type

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;
use tracing::{debug, info, warn};
use uplink::{read_frame, write_frame};

use crate::hub::Link;
use crate::radio::Inbound;

/// Phones connected at once (a household or farm has a handful).
const MAX_CONNECTIONS: usize = 16;
/// Close a connection that sends nothing for this long. The app polls every
/// few seconds while a screen is open and reconnects when needed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Replies waiting to be written to one phone before more are dropped.
const WRITE_QUEUE: usize = 8;

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

struct Conn {
    tx: mpsc::Sender<Vec<u8>>,
    opened: Instant,
    abort: Option<AbortHandle>,
}

/// The open phone connections, shared with the main loop.
#[derive(Clone, Default)]
pub struct Phones(Arc<Mutex<HashMap<u64, Conn>>>);

impl Phones {
    /// Queues `frame` for connection `id`. Dropped if it's gone or backed up;
    /// the phone times out and asks again.
    pub fn send(&self, id: u64, frame: Vec<u8>) {
        let map = self.0.lock().unwrap();
        match map.get(&id) {
            Some(c) => {
                if c.tx.try_send(frame).is_err() {
                    debug!("phone connection {id} is backed up; reply dropped");
                }
            }
            None => debug!("phone connection {id} closed before its reply"),
        }
    }

    fn admit(&self, id: u64, tx: mpsc::Sender<Vec<u8>>) {
        let mut map = self.0.lock().unwrap();
        if map.len() >= MAX_CONNECTIONS
            && let Some(oldest) = map.iter().min_by_key(|(_, c)| c.opened).map(|(id, _)| *id)
            && let Some(c) = map.remove(&oldest)
        {
            if let Some(h) = c.abort {
                h.abort();
            }
            debug!("phone port full; closed connection {oldest}");
        }
        map.insert(
            id,
            Conn {
                tx,
                opened: Instant::now(),
                abort: None,
            },
        );
    }

    fn set_abort(&self, id: u64, h: AbortHandle) {
        if let Some(c) = self.0.lock().unwrap().get_mut(&id) {
            c.abort = Some(h);
        }
    }

    fn release(&self, id: u64) {
        self.0.lock().unwrap().remove(&id);
    }
}

/// Accepts phone connections for as long as the hub runs.
pub async fn serve(listener: TcpListener, inbound: mpsc::Sender<Inbound>, phones: Phones) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = mpsc::channel(WRITE_QUEUE);
                phones.admit(id, tx);
                let ph = phones.clone();
                let inb = inbound.clone();
                let task = tokio::spawn(async move {
                    debug!("phone connection {id} from {peer}");
                    if let Err(e) = handle(stream, id, inb, rx).await {
                        debug!("phone connection {id} closed: {e:#}");
                    }
                    ph.release(id);
                });
                phones.set_abort(id, task.abort_handle());
            }
            Err(e) => warn!("phone accept: {e}"),
        }
    }
}

async fn handle(
    stream: TcpStream,
    id: u64,
    inbound: mpsc::Sender<Inbound>,
    mut replies: mpsc::Receiver<Vec<u8>>,
) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    let (mut rd, mut wr) = stream.into_split();
    // The writer stops when this connection's reply channel closes.
    let writer = tokio::spawn(async move {
        while let Some(f) = replies.recv().await {
            if write_frame(&mut wr, &f).await.is_err() {
                break;
            }
        }
    });
    let result = async {
        loop {
            let frame = tokio::time::timeout(IDLE_TIMEOUT, read_frame(&mut rd))
                .await
                .map_err(|_| anyhow::anyhow!("idle"))??;
            inbound
                .send(Inbound {
                    frame,
                    link: Link::App(id),
                })
                .await
                .map_err(|_| anyhow::anyhow!("hub stopped"))?;
        }
    }
    .await;
    writer.abort();
    result
}

/// Binds the phone port.
pub async fn bind(addr: &str) -> anyhow::Result<TcpListener> {
    let l = TcpListener::bind(addr).await?;
    info!("listening for the phone app on tcp://{addr}");
    Ok(l)
}
