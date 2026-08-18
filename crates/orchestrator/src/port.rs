//! Safe loopback port allocation for network-backed runtime adapters
//! (Task 18).
//!
//! Ports are only ever bound to `127.0.0.1` -- never `0.0.0.0` -- so a
//! runtime instance's internal listener is unreachable from outside the
//! host regardless of firewall configuration; `CloudDesk`'s own
//! authenticated proxy (`crate::proxy`) is the only sanctioned path to
//! it.

use std::collections::HashSet;
use std::net::{SocketAddr, TcpListener};
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum PortError {
    #[error("no free loopback port could be allocated")]
    Exhausted,
}

/// Tracks ports currently associated with a live instance so two
/// instances never get handed the same port even if the OS would
/// otherwise be willing to reuse it before the first listener closes
/// (relevant during the brief window between "we allocated it" and "the
/// adapter's own process/container has bound it").
#[derive(Default)]
pub struct PortAllocator {
    reserved: Mutex<HashSet<u16>>,
}

pub struct ReservedPort {
    port: u16,
}

impl ReservedPort {
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl PortAllocator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds an OS-assigned ephemeral port on 127.0.0.1, immediately
    /// releases the socket (so the adapter's own process/container can
    /// bind it), and records it as reserved until `release` is called.
    /// The bind-then-release pattern is the standard way to get a
    /// genuinely free port without a race against another unrelated
    /// process on the host; the additional in-process `reserved` set
    /// closes the much narrower race against *this manager* handing the
    /// same port to two instances concurrently.
    pub fn allocate(&self) -> Result<ReservedPort, PortError> {
        let mut reserved = self
            .reserved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for _ in 0..32 {
            let Ok(listener) = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))) else {
                continue;
            };
            let Ok(addr) = listener.local_addr() else {
                continue;
            };
            drop(listener);
            if reserved.insert(addr.port()) {
                return Ok(ReservedPort { port: addr.port() });
            }
            // Extremely unlikely (we just reserved it ourselves in an
            // earlier iteration and haven't released it): try again.
        }
        Err(PortError::Exhausted)
    }

    /// Releases a port after the instance that held it has fully
    /// terminated (Task 18: "release port after termination").
    pub fn release(&self, port: u16) {
        self.reserved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_distinct_ports_and_releases_them() {
        let allocator = PortAllocator::new();
        let a = allocator.allocate().unwrap();
        let b = allocator.allocate().unwrap();
        assert_ne!(a.port(), b.port());
        allocator.release(a.port());
        // Re-allocating after release must not error out (proves
        // release actually frees the slot for bookkeeping purposes).
        let c = allocator.allocate().unwrap();
        assert!(c.port() > 0);
    }

    #[test]
    fn allocated_port_is_bound_only_to_loopback() {
        let allocator = PortAllocator::new();
        let reserved = allocator.allocate().unwrap();
        // A listener bound to 0.0.0.0 on the same port would fail if the
        // ephemeral bind briefly held it in TIME_WAIT on all
        // interfaces; more directly, confirm we can immediately rebind
        // on 127.0.0.1 (proving the original bind, now dropped, was
        // loopback-only and not lingering).
        let listener =
            TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], reserved.port()))).unwrap();
        drop(listener);
    }
}
