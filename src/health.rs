//! `loxone-audioserver health`: a self-check for container healthchecks. The image has no
//! shell or HTTP client, so the binary checks that the daemon accepts TCP connections on the
//! remote (7091) and miniserver (7095) ports on loopback.

use std::time::Duration;

use anyhow::{bail, Result};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::command_audit::{PORT_MINISERVER, PORT_REMOTE};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn check() -> Result<()> {
    check_ports("127.0.0.1", &[PORT_REMOTE, PORT_MINISERVER]).await
}

async fn check_ports(host: &str, ports: &[u16]) -> Result<()> {
    for &port in ports {
        match timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => bail!("port {port} not accepting connections: {e}"),
            Err(_) => bail!("port {port} did not answer within {CONNECT_TIMEOUT:?}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn listening_ports_are_healthy() {
        let a = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let b = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ports = [
            a.local_addr().unwrap().port(),
            b.local_addr().unwrap().port(),
        ];
        check_ports("127.0.0.1", &ports).await.unwrap();
    }

    #[tokio::test]
    async fn closed_port_is_unhealthy() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let err = check_ports("127.0.0.1", &[port]).await.unwrap_err();
        assert!(err.to_string().contains(&port.to_string()));
    }
}
