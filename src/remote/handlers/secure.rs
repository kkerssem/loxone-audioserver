//! Secure auth and getkey handlers (7091 Web UI bootstrap).
//!
//! Port **7091** `secure/authenticate` satisfies the Loxone App login handshake only — it is an
//! **intentional stub** and is not an authorization boundary. Real pairing, JWT validation, and
//! RSA session-key exchange live on port **7095** in [`secure_session.rs`](../../miniserver/secure_session.rs).
//! See [web-ui-protocol.md](../../../docs/web-ui-protocol.md#secureauthenticateusertupleciphertext).
//!
//! **Future work** (not implemented): decrypt RSA tuple, AES-CBC ciphertext, validate
//! `session_token`, Miniserver `jdev/sys/checktoken/{token}/{user}`, persist authenticated user.
use base64::prelude::*;
use matchit::Params;
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use serde_json::json;

use anyhow::Context;

use crate::miniserver;
use crate::wire_response::build_wire_result_response;

use super::super::Api;
use tracing::debug;

impl Api {
    pub(in crate::remote) fn authenticate(
        &self,
        command: &str,
        params: &Params,
    ) -> anyhow::Result<String> {
        let _user = params.get("user").context("Missing parameter user")?;
        let _tuple = params.get("tuple").context("Missing parameter tuple")?;
        let _ciphertext = params
            .get("ciphertext")
            .context("Missing parameter ciphertext")?;
        // Intentional stub — see module docs and docs/web-ui-protocol.md § secure/authenticate.
        build_wire_result_response(
            "authenticate_result",
            json!("authentication successful"),
            Some(command),
            &[],
        )
    }

    pub(in crate::remote) fn authenticate_false(
        &self,
        command: &str,
        params: &Params,
    ) -> anyhow::Result<String> {
        let _user = params.get("user").context("Missing parameter user")?;
        let _signature = params
            .get("signature")
            .context("Missing parameter signature")?;
        // Intentional stub logout — same security model as authenticate.
        build_wire_result_response("authenticate_result", json!([]), Some(command), &[])
    }

    pub(in crate::remote) fn get_key(&self, command: &str) -> anyhow::Result<String> {
        self.get_key_response(command, false)
    }

    pub(in crate::remote) fn get_key_full(&self, command: &str) -> anyhow::Result<String> {
        self.get_key_response(command, true)
    }

    pub(in crate::remote) fn get_key_response(
        &self,
        command: &str,
        full: bool,
    ) -> anyhow::Result<String> {
        let public_key = &self.app_config.key_pair.1;
        if let Some(fp) = miniserver::audioserver_spki_fingerprint(public_key) {
            debug!(
                port = 7091,
                api = "remote",
                audioserver_pubkey_fp = %fp,
                full,
                "remote getkey served audioserver pubkey"
            );
        }
        let public_key_pem = public_key.to_public_key_pem(LineEnding::default())?;
        let entry = if full {
            json!({ "pubkey": public_key_pem })
        } else {
            json!({
                "pubkey": BASE64_STANDARD_NO_PAD.encode(&public_key_pem),
                "exp": 65537,
            })
        };
        build_wire_result_response("getkey_result", json!([entry]), Some(command), &[])
    }
}
