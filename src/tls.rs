//! TLS termination for Loxone ports (7095 Miniserver, 7091 remote).
//!
//! Deferred: some Gen-2 Miniservers require HTTPS/WebSocket over TLS on these ports.
//! See [lox-audioserver protocolDispatcher](https://github.com/lox-audioserver/lox-audioserver/blob/dev/src/adapters/loxone/http/protocolDispatcher.ts).
//! Plain WebSocket on 7095 is used for initial pairing; enable TLS here when pairing fails.
