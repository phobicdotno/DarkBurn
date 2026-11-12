//! A Ruida emulator behind real UDP sockets on 127.0.0.1, for tests of [`UdpTransport`] and of
//! everything above it (checksums, ACKs, timeouts, the jog port) over the operating system's
//! network stack.
//!
//! The server binds two ports the system picks on the loopback address only, so nothing can
//! reach it from another computer and it never talks to one. A real controller listens on
//! 50200/50207 and answers to 40200/40207; the server answers to wherever a datagram came from.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::emulator::RuidaEmulator;
use crate::error::Result;
use crate::transport::{UdpPorts, UdpTransport};

/// The emulator served over UDP on 127.0.0.1. Stops when dropped.
pub struct UdpEmulatorServer {
    emu: Arc<Mutex<RuidaEmulator>>,
    data: SocketAddr,
    jog: SocketAddr,
    stop: Arc<AtomicBool>,
    drop_replies: Arc<AtomicUsize>,
    datagrams: Arc<AtomicUsize>,
    threads: Vec<JoinHandle<()>>,
}

impl UdpEmulatorServer {
    /// Bind two free loopback ports and serve `emu` on them.
    pub fn start(emu: Arc<Mutex<RuidaEmulator>>) -> std::io::Result<Self> {
        let data = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let jog = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let (data_addr, jog_addr) = (data.local_addr()?, jog.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let drop_replies = Arc::new(AtomicUsize::new(0));
        let datagrams = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for (socket, is_jog) in [(data, false), (jog, true)] {
            let (emu, stop, drops, count) = (emu.clone(), stop.clone(), drop_replies.clone(), datagrams.clone());
            threads.push(std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                loop {
                    // A plain blocking read, woken by a datagram from `Drop`: no polling, so a
                    // datagram is answered as soon as the system hands it over.
                    let Ok((n, from)) = socket.recv_from(&mut buf) else {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        continue;
                    };
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // Loopback only: anything else is not ours to answer.
                    if !from.ip().is_loopback() {
                        continue;
                    }
                    count.fetch_add(1, Ordering::Relaxed);
                    let replies = {
                        let mut e = emu.lock().unwrap_or_else(|p| p.into_inner());
                        if is_jog { e.handle_jog_datagram(&buf[..n]) } else { e.handle_udp_datagram(&buf[..n]) }
                    };
                    for r in replies {
                        if take_one(&drops) {
                            continue;
                        }
                        let _ = socket.send_to(&r, from);
                    }
                }
            }));
        }
        Ok(UdpEmulatorServer { emu, data: data_addr, jog: jog_addr, stop, drop_replies, datagrams, threads })
    }

    pub fn emulator(&self) -> Arc<Mutex<RuidaEmulator>> {
        self.emu.clone()
    }

    /// The loopback ports the emulator listens on, with host ports left to the system.
    pub fn ports(&self) -> UdpPorts {
        UdpPorts { data: self.data.port(), jog: self.jog.port(), host_data: 0, host_jog: 0 }
    }

    /// A UDP transport connected to this server (127.0.0.1 only).
    pub fn transport(&self) -> Result<UdpTransport> {
        UdpTransport::connect_ports(Ipv4Addr::LOCALHOST.into(), self.ports())
    }

    /// Swallow the next `n` datagrams the emulator would send back (a lossy network).
    pub fn drop_replies(&self, n: usize) {
        self.drop_replies.store(n, Ordering::Relaxed);
    }

    /// Datagrams received so far on both ports.
    pub fn datagrams(&self) -> usize {
        self.datagrams.load(Ordering::Relaxed)
    }
}

/// Count `n` down by one unless it is zero; true when it was counted down.
fn take_one(n: &AtomicUsize) -> bool {
    let mut cur = n.load(Ordering::Relaxed);
    while cur > 0 {
        match n.compare_exchange(cur, cur - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(now) => cur = now,
        }
    }
    false
}

impl Drop for UdpEmulatorServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake both threads out of their blocking reads with a datagram each.
        let woke = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|s| {
            s.send_to(&[0], self.data)?;
            s.send_to(&[0], self.jog)?;
            Ok(())
        });
        for t in self.threads.drain(..) {
            // If the wake-up could not be sent the threads stay asleep; do not wait for them.
            if woke.is_ok() {
                let _ = t.join();
            }
        }
    }
}
