//! Bug hunt 10: the five seconds a new connection has for its hello cover the whole hello, not
//! each read of it.

use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use darkburn_mcp::control::{ControlConfig, ControlServer};

#[test]
fn a_hello_trickled_one_byte_at_a_time_is_cut_off_after_five_seconds() {
    let server = ControlServer::bind(ControlConfig { port: 0, token: Some("test-token-0123456789abcdef".into()), ..Default::default() }).unwrap();
    let s = TcpStream::connect(server.addr()).unwrap();
    let start = Instant::now();
    // A frame that claims 100 bytes, sent one byte every 700 ms: no single read waits five
    // seconds, so the connection was kept (unauthenticated) for as long as the client liked.
    let stop = Arc::new(AtomicBool::new(false));
    let (stop2, mut w) = (stop.clone(), s.try_clone().unwrap());
    let writer = std::thread::spawn(move || {
        let mut bytes = 100u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(&[b'x'; 100]);
        for b in bytes {
            if stop2.load(Ordering::SeqCst) || w.write_all(&[b]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(700));
        }
    });
    while server.refused_connections() == 0 && start.elapsed() < Duration::from_millis(6500) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let waited = start.elapsed();
    stop.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    assert_eq!(server.refused_connections(), 1, "the slow hello still held its connection after {waited:?}");
    assert!(waited >= Duration::from_millis(4500), "refused too early: {waited:?}");
}
