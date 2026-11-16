//! Secure pairing session state for Miniserver `secure/hello` and `secure/init`.

use base64::prelude::*;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
#[cfg(test)]
use jsonwebtoken::{encode, EncodingKey, Header};
#[cfg(test)]
use rsa::pkcs8::EncodePrivateKey;
use rsa::{
    pkcs1::DecodeRsaPublicKey,
    pkcs8::{DecodePublicKey, EncodePublicKey, LineEnding},
    traits::PublicKeyParts,
    Oaep, Pkcs1v15Encrypt, PrivateKey as RsaPrivateKey, PublicKey as RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::debug;
use x509_cert::{
    certificate::Certificate,
    der::{Decode, Encode},
};

pub const INIT_SUB: &str = "secure-session-init";
#[cfg(test)]
const INIT_SUCCESS_SUB: &str = "secure-session-init-success";
#[cfg(test)]
const SUCCESS_TTL_SECS: i64 = 60;
/// Grace period for request JWT `exp` when validating (clock skew).
pub const EXP_GRACE_SECS: i64 = 300;

const RSA_CIPHERTEXT_LEN: usize = 256;
const SESSION_KEYS_CIPHER_LEN: usize = 512;

/// Success JWT returned by reference lox-audioserver and accepted by Miniserver in traces.
pub const LEGACY_JWT: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.ewogICJleHAiOiAxNjQwNjEwMTQ0LAogICJpYXQiOiAxNjQwNjEwMDg0LAogICJzZXNzaW9uX3Rva2VuIjogIjhXYWh3QWZVTHdFUWNlOVl1MHFJRTlMN1FNa1hGSGJpME05Y2g5dktjZ1lBclBQb2pYSHBTaU5jcTBmVDNscUwiLAogICJzdWIiOiAic2VjdXJlLXNlc3Npb24taW5pdC1zdWNjZXNzIgp9.Zd5M55YPirdugqlGr7u6iB-kM_oFqnvMnpxL8gj58vF2L4ocpSY6S8OB_4f8LeIB2AIYikN5U6R0UALJ3Oahxa0gq9qKDoNrjC7-Q8wAe1rEhDbvdWtaRzmgiHnivrz0cNsyeYGBX8c5Ix6pLI8URGjR1Ox2lbxBt_pVZ-MyEvhVNSJ0-DttclqIAgr_24tVmwe6lleT5eKyBoQVAcGJP-3LSdORKckHTCRw6aaf6sOQ7AtK37SXgnHB6J4g2wErvyw29mMAmDTbR8vZUCmTxgnmhbrks02AZITLaDeGAYTlSASWDSl84L9wkWOWk0pufZIGG0zcXgL8EoWD8cw_fIhbh-LXODEY5251u0DlVtaI_6J6o2j8jy_WvsSqKh-sqqy-ygScwPkLgFua7GNlppaHUGsFaEg0rVdLvVAiIV3mbOGnis1RuWcTWY9iuPVxFTODxkOZNRgZttBb_NFa8lQPJKwwhA33YC1hJ6DE3xEC2rvc4LGE400nLKnELNKpFNsom07JFSQQq8NV3Z1lzTksa8ANdXrV080J8x0c1Bt4dcUyx3lzFE8XG3DsLXCnL2YsJ9ik2jdSBZL8grnoQjqvJWaX3j47P0VM-jaMICVb6QcVP-nNB7k5n1qQGASsbkhcB1nffzE_wLooUe4iLxJQ2dkCM1n7ngXDF6HK0_A";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecryptScheme {
    Pkcs1v15,
    OaepSha1,
    OaepSha256,
}

impl DecryptScheme {
    /// OAEP first: since rsa 0.10, PKCS#1 v1.5 decryption never fails on bad padding (implicit
    /// rejection, the Marvin mitigation) and returns synthetic plaintext instead, so it can only
    /// be tried last and its result must look like session keys (see [`decrypt_blocks_mixed`]).
    fn all() -> [Self; 3] {
        [Self::OaepSha1, Self::OaepSha256, Self::Pkcs1v15]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pkcs1v15 => "PKCS#1 v1.5",
            Self::OaepSha1 => "OAEP-SHA1",
            Self::OaepSha256 => "OAEP-SHA256",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecryptedSessionKeys {
    pub raw: Vec<u8>,
    pub scheme: DecryptScheme,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SecureInitRequest {
    pub sub: String,
    pub session_token: String,
    pub session_keys: String,
    pub encryption_type: u32,
    pub iat: i64,
    pub exp: i64,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SecureInitSuccess {
    pub sub: String,
    pub session_token: String,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecureHelloState {
    pub client_time: String,
    pub cert_chain_b64: String,
}

#[derive(Clone, Debug, Default)]
pub struct SecureSessionState {
    pub expected_session_token: String,
    pub hello: Option<SecureHelloState>,
    pub last_init: Option<SecureInitRequest>,
    pub decrypted_keys: Option<DecryptedSessionKeys>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SecureInitError {
    InvalidJwtFormat,
    InvalidJwtPayload(String),
    WrongSub { expected: String, got: String },
    TokenMismatch,
    Expired { exp: i64, now: i64 },
    VerifyFailed(String),
    CertParseFailed(String),
    DecryptFailed(String),
}

impl fmt::Display for SecureInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJwtFormat => write!(f, "JWT must have header.payload.signature"),
            Self::InvalidJwtPayload(msg) => write!(f, "invalid JWT payload: {msg}"),
            Self::WrongSub { expected, got } => {
                write!(f, "unexpected sub: expected {expected}, got {got}")
            }
            Self::TokenMismatch => write!(f, "session_token does not match greeting"),
            Self::Expired { exp, now } => write!(f, "init JWT expired (exp={exp}, now={now})"),
            Self::VerifyFailed(msg) => write!(f, "JWT signature verification failed: {msg}"),
            Self::CertParseFailed(msg) => write!(f, "certificate parse failed: {msg}"),
            Self::DecryptFailed(msg) => write!(f, "session_keys decrypt failed: {msg}"),
        }
    }
}

impl std::error::Error for SecureInitError {}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

pub fn is_probe_session_keys_decrypt() -> bool {
    env_flag("PROBE_SESSION_KEYS_DECRYPT")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelloCertKeyInfo {
    pub index: usize,
    pub subject_cn: String,
    pub spki_fp: String,
    pub rsa_bits: u32,
    pub verifies_init_jwt: bool,
    pub matches_audioserver_pubkey: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecryptLayout {
    SingleBlock,
    SwappedBlocks,
}

impl DecryptLayout {
    pub fn label(self) -> &'static str {
        match self {
            Self::SingleBlock => "single_256",
            Self::SwappedBlocks => "swapped_2x256",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecryptAttempt {
    pub layout: DecryptLayout,
    pub scheme: DecryptScheme,
    pub scheme_block1: Option<DecryptScheme>,
    pub success: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaintextShape {
    HexColonHex,
    Binary,
    Empty,
}

impl PlaintextShape {
    pub fn label(self) -> &'static str {
        match self {
            Self::HexColonHex => "hex_colon_hex",
            Self::Binary => "binary",
            Self::Empty => "empty",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionKeysDecryptProbe {
    pub audioserver_pubkey_fp: String,
    pub encryption_type: u32,
    pub chain_keys: Vec<HelloCertKeyInfo>,
    pub jwt_signer_index: Option<usize>,
    pub decrypted: Option<DecryptedSessionKeys>,
    pub plaintext_shape: Option<PlaintextShape>,
    pub attempts: Vec<DecryptAttempt>,
    pub primary_error: Option<String>,
}

/// Short fingerprint of the audioserver SPKI for decrypt diagnostics.
pub fn public_key_fingerprint(public_key: &RsaPublicKey) -> Result<String, SecureInitError> {
    let spki = public_key
        .to_public_key_der()
        .map_err(|e| SecureInitError::CertParseFailed(e.to_string()))?;
    let hash = Sha256::digest(&spki);
    Ok(hash[..8].iter().map(|byte| format!("{byte:02x}")).collect())
}

fn decode_jwt_segment(segment: &str) -> Result<Vec<u8>, SecureInitError> {
    BASE64_URL_SAFE_NO_PAD
        .decode(segment.as_bytes())
        .map_err(|e| SecureInitError::InvalidJwtPayload(e.to_string()))
}

pub fn parse_jwt_payload_unverified(token: &str) -> Result<SecureInitRequest, SecureInitError> {
    let mut parts = token.split('.');
    let _header = parts.next().ok_or(SecureInitError::InvalidJwtFormat)?;
    let payload_b64 = parts.next().ok_or(SecureInitError::InvalidJwtFormat)?;
    let _signature = parts.next().ok_or(SecureInitError::InvalidJwtFormat)?;
    if parts.next().is_some() {
        return Err(SecureInitError::InvalidJwtFormat);
    }

    let payload_bytes = decode_jwt_segment(payload_b64)?;
    serde_json::from_slice(&payload_bytes)
        .map_err(|e| SecureInitError::InvalidJwtPayload(e.to_string()))
}

pub fn parse_cert_chain_pem(cert_chain_b64: &str) -> Result<Vec<Certificate>, SecureInitError> {
    let pem_bytes = BASE64_STANDARD
        .decode(cert_chain_b64.trim())
        .map_err(|e| SecureInitError::CertParseFailed(e.to_string()))?;
    let pem = std::str::from_utf8(&pem_bytes)
        .map_err(|e| SecureInitError::CertParseFailed(e.to_string()))?;

    let mut certs = Vec::new();
    for block in pem.split("-----BEGIN CERTIFICATE-----") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let der_b64: String = block
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let der = BASE64_STANDARD
            .decode(der_b64.as_bytes())
            .map_err(|e| SecureInitError::CertParseFailed(e.to_string()))?;
        certs.push(
            Certificate::from_der(&der)
                .map_err(|e| SecureInitError::CertParseFailed(e.to_string()))?,
        );
    }

    if certs.is_empty() {
        return Err(SecureInitError::CertParseFailed(
            "no certificates in hello chain".into(),
        ));
    }
    Ok(certs)
}

fn rsa_public_key_from_cert(cert: &Certificate) -> Result<RsaPublicKey, SecureInitError> {
    let spki = &cert.tbs_certificate().subject_public_key_info();
    let spki_der = spki
        .to_der()
        .map_err(|e| SecureInitError::CertParseFailed(e.to_string()))?;
    if let Ok(key) = RsaPublicKey::from_public_key_der(&spki_der) {
        return Ok(key);
    }
    let pkcs1 = spki.subject_public_key.raw_bytes();
    RsaPublicKey::from_pkcs1_der(pkcs1).map_err(|e| SecureInitError::CertParseFailed(e.to_string()))
}

/// Miniserver signing key from the leaf certificate in the hello chain.
#[cfg(test)]
fn miniserver_verify_key(chain: &[Certificate]) -> Result<RsaPublicKey, SecureInitError> {
    let cert = chain
        .first()
        .ok_or_else(|| SecureInitError::CertParseFailed("empty certificate chain".into()))?;
    rsa_public_key_from_cert(cert)
}

fn find_verify_key_in_chain(chain: &[Certificate], jwt: &str) -> Option<RsaPublicKey> {
    for cert in chain {
        if let Ok(key) = rsa_public_key_from_cert(cert) {
            if verify_init_jwt(jwt, &key).is_ok() {
                return Some(key);
            }
        }
    }
    None
}

#[cfg(test)]
fn find_verify_key_for_jwt(chain: &[Certificate], jwt: &str) -> Option<RsaPublicKey> {
    find_verify_key_in_chain(chain, jwt)
}

pub fn verify_init_jwt(
    token: &str,
    public_key: &RsaPublicKey,
) -> Result<SecureInitRequest, SecureInitError> {
    verify_jwt_rs256(token, public_key)
}

#[cfg(test)]
fn verify_success_jwt(
    token: &str,
    public_key: &RsaPublicKey,
) -> Result<SecureInitSuccess, SecureInitError> {
    verify_jwt_rs256(token, public_key)
}

fn verify_jwt_rs256<T: for<'de> Deserialize<'de>>(
    token: &str,
    public_key: &RsaPublicKey,
) -> Result<T, SecureInitError> {
    let pem = public_key
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| SecureInitError::VerifyFailed(e.to_string()))?;
    let key = DecodingKey::from_rsa_pem(pem.as_bytes())
        .map_err(|e| SecureInitError::VerifyFailed(e.to_string()))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.set_required_spec_claims::<&str>(&[]);

    let data = decode::<T>(token, &key, &validation)
        .map_err(|e| SecureInitError::VerifyFailed(e.to_string()))?;
    Ok(data.claims)
}

/// Prefer RS256 verify when hello cert is present; fall back to unverified parse (legacy compat).
pub fn resolve_init_request(
    jwt: &str,
    state: &SecureSessionState,
) -> Result<SecureInitRequest, SecureInitError> {
    if let Some(hello) = &state.hello {
        if let Ok(chain) = parse_cert_chain_pem(&hello.cert_chain_b64) {
            if let Some(ms_key) = find_verify_key_in_chain(&chain, jwt) {
                match verify_init_jwt(jwt, &ms_key) {
                    Ok(req) => return Ok(req),
                    Err(error) => {
                        debug!(
                            error = %error,
                            "secure/init JWT verify failed; falling back to unverified parse"
                        );
                    }
                }
            } else {
                debug!(
                    "secure/init JWT did not verify against hello cert chain; falling back to unverified parse"
                );
            }
        }
    } else {
        debug!("secure/init without prior secure/hello; parsing JWT without signature verify");
    }
    parse_jwt_payload_unverified(jwt)
}

pub fn validate_init_request(
    req: &SecureInitRequest,
    expected: &str,
    now: SystemTime,
) -> Result<(), SecureInitError> {
    if req.sub != INIT_SUB {
        return Err(SecureInitError::WrongSub {
            expected: INIT_SUB.to_string(),
            got: req.sub.clone(),
        });
    }
    if req.session_token != expected {
        return Err(SecureInitError::TokenMismatch);
    }
    let now_secs = system_time_secs(now);
    if req.exp + EXP_GRACE_SECS <= now_secs {
        return Err(SecureInitError::Expired {
            exp: req.exp,
            now: now_secs,
        });
    }
    Ok(())
}

#[cfg(test)]
fn build_success_claims(req: &SecureInitRequest, now: SystemTime) -> SecureInitSuccess {
    let iat = system_time_secs(now);
    SecureInitSuccess {
        sub: INIT_SUCCESS_SUB.to_string(),
        session_token: req.session_token.clone(),
        iat,
        exp: iat + SUCCESS_TTL_SECS,
    }
}

#[cfg(test)]
fn sign_success_jwt(
    success: &SecureInitSuccess,
    private_key: &RsaPrivateKey,
) -> Result<String, SecureInitError> {
    let pem = private_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| SecureInitError::VerifyFailed(e.to_string()))?;
    let key = EncodingKey::from_rsa_pem(pem.as_bytes())
        .map_err(|e| SecureInitError::VerifyFailed(e.to_string()))?;
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("JWT".into());
    encode(&header, success, &key).map_err(|e| SecureInitError::VerifyFailed(e.to_string()))
}

pub fn legacy_success_jwt() -> &'static str {
    LEGACY_JWT
}

pub fn decrypt_session_keys(
    ciphertext_b64: &str,
    private_key: &RsaPrivateKey,
    encryption_type: u32,
) -> Result<DecryptedSessionKeys, SecureInitError> {
    let cipher = BASE64_STANDARD
        .decode(ciphertext_b64.trim())
        .map_err(|e| SecureInitError::DecryptFailed(e.to_string()))?;
    if cipher.len() != SESSION_KEYS_CIPHER_LEN {
        return Err(SecureInitError::DecryptFailed(format!(
            "expected {SESSION_KEYS_CIPHER_LEN} bytes, got {}",
            cipher.len()
        )));
    }

    let schemes = schemes_for_encryption_type(encryption_type);
    for scheme in schemes {
        if let Ok(raw) = decrypt_blocks_uniform(private_key, &cipher, scheme) {
            return Ok(DecryptedSessionKeys { raw, scheme });
        }
    }

    let blocks = split_rsa_blocks(&cipher)?;
    for scheme0 in DecryptScheme::all() {
        for scheme1 in DecryptScheme::all() {
            if let Ok(raw) = decrypt_blocks_mixed(private_key, &blocks, [scheme0, scheme1]) {
                return Ok(DecryptedSessionKeys {
                    raw,
                    scheme: scheme0,
                });
            }
        }
    }

    let fingerprint = public_key_fingerprint(&private_key.to_public_key())?;
    Err(SecureInitError::DecryptFailed(format!(
        "no RSA scheme decrypted session_keys (encryption_type={encryption_type}, audioserver_pubkey_fp={fingerprint}; Miniserver may be encrypting with a different audioserver public key than private.pem)"
    )))
}

fn schemes_for_encryption_type(encryption_type: u32) -> Vec<DecryptScheme> {
    match encryption_type {
        // Observed in traces; Java-style RSA often uses PKCS#1 v1.5 or OAEP-SHA1. PKCS#1 v1.5
        // goes last (see `DecryptScheme::all`).
        1 => vec![
            DecryptScheme::OaepSha1,
            DecryptScheme::OaepSha256,
            DecryptScheme::Pkcs1v15,
        ],
        _ => DecryptScheme::all().into(),
    }
}

fn split_rsa_blocks(cipher: &[u8]) -> Result<[&[u8]; 2], SecureInitError> {
    if cipher.len() != SESSION_KEYS_CIPHER_LEN {
        return Err(SecureInitError::DecryptFailed(format!(
            "expected {SESSION_KEYS_CIPHER_LEN} bytes, got {}",
            cipher.len()
        )));
    }
    Ok([&cipher[..RSA_CIPHERTEXT_LEN], &cipher[RSA_CIPHERTEXT_LEN..]])
}

fn decrypt_blocks_uniform(
    private_key: &RsaPrivateKey,
    cipher: &[u8],
    scheme: DecryptScheme,
) -> Result<Vec<u8>, rsa::errors::Error> {
    let blocks = [&cipher[..RSA_CIPHERTEXT_LEN], &cipher[RSA_CIPHERTEXT_LEN..]];
    decrypt_blocks_mixed(private_key, &blocks, [scheme, scheme])
}

fn decrypt_blocks_mixed(
    private_key: &RsaPrivateKey,
    blocks: &[&[u8]; 2],
    schemes: [DecryptScheme; 2],
) -> Result<Vec<u8>, rsa::errors::Error> {
    let mut out = Vec::new();
    for (block, scheme) in blocks.iter().zip(schemes) {
        out.extend_from_slice(&decrypt_rsa_block(private_key, block, scheme)?);
    }
    reject_unverified_pkcs1v15(out, &schemes)
}

/// Single RSA block (probe layout), with the same PKCS#1 v1.5 check as [`decrypt_blocks_mixed`].
fn decrypt_single_block(
    private_key: &RsaPrivateKey,
    block: &[u8],
    scheme: DecryptScheme,
) -> Result<Vec<u8>, rsa::errors::Error> {
    reject_unverified_pkcs1v15(decrypt_rsa_block(private_key, block, scheme)?, &[scheme])
}

/// PKCS#1 v1.5 cannot report bad padding (rsa 0.10 implicit rejection returns synthetic
/// plaintext, e.g. for the wrong key or an OAEP ciphertext). Its output only counts when it has
/// the session-key shape (`hex:hex`); OAEP results are authenticated by their padding check.
fn reject_unverified_pkcs1v15(
    raw: Vec<u8>,
    schemes: &[DecryptScheme],
) -> Result<Vec<u8>, rsa::errors::Error> {
    if schemes.contains(&DecryptScheme::Pkcs1v15)
        && classify_plaintext_shape(&raw) != PlaintextShape::HexColonHex
    {
        return Err(rsa::errors::Error::Decryption);
    }
    Ok(raw)
}

fn decrypt_rsa_block(
    private_key: &RsaPrivateKey,
    block: &[u8],
    scheme: DecryptScheme,
) -> Result<Vec<u8>, rsa::errors::Error> {
    match scheme {
        DecryptScheme::Pkcs1v15 => private_key.decrypt(Pkcs1v15Encrypt, block),
        DecryptScheme::OaepSha1 => private_key.decrypt(Oaep::<sha1::Sha1>::new(), block),
        DecryptScheme::OaepSha256 => private_key.decrypt(Oaep::<sha2::Sha256>::new(), block),
    }
}

fn find_verify_key_index_in_chain(chain: &[Certificate], jwt: &str) -> Option<usize> {
    for (index, cert) in chain.iter().enumerate() {
        if let Ok(key) = rsa_public_key_from_cert(cert) {
            if verify_init_jwt(jwt, &key).is_ok() {
                return Some(index);
            }
        }
    }
    None
}

fn extract_subject_cn(cert: &Certificate) -> String {
    // `Name::iter` walks every attribute of every RDN, in order.
    for attr in cert.tbs_certificate().subject().iter() {
        if attr.oid.to_string() == "2.5.4.3" {
            if let Ok(text) = std::str::from_utf8(attr.value.value()) {
                return text.to_string();
            }
            return format!("{:?}", attr.value);
        }
    }
    "unknown".to_string()
}

fn rsa_key_bits(public_key: &RsaPublicKey) -> u32 {
    public_key.n().bit_width()
}

pub fn summarize_hello_cert_chain(
    chain: &[Certificate],
    jwt: &str,
    audioserver_public: &RsaPublicKey,
) -> Result<Vec<HelloCertKeyInfo>, SecureInitError> {
    let audioserver_fp = public_key_fingerprint(audioserver_public)?;
    let mut keys = Vec::with_capacity(chain.len());
    for (index, cert) in chain.iter().enumerate() {
        let public_key = rsa_public_key_from_cert(cert)?;
        let spki_fp = public_key_fingerprint(&public_key)?;
        let matches_audioserver_pubkey = spki_fp == audioserver_fp;
        keys.push(HelloCertKeyInfo {
            index,
            subject_cn: extract_subject_cn(cert),
            spki_fp,
            rsa_bits: rsa_key_bits(&public_key),
            verifies_init_jwt: verify_init_jwt(jwt, &public_key).is_ok(),
            matches_audioserver_pubkey,
        });
    }
    Ok(keys)
}

pub fn classify_plaintext_shape(raw: &[u8]) -> PlaintextShape {
    if raw.is_empty() {
        return PlaintextShape::Empty;
    }
    if let Ok(text) = std::str::from_utf8(raw) {
        let mut parts = text.split(':');
        let left = parts.next().unwrap_or("");
        let right = parts.next().unwrap_or("");
        if parts.next().is_none()
            && !left.is_empty()
            && !right.is_empty()
            && left.chars().all(|c| c.is_ascii_hexdigit())
            && right.chars().all(|c| c.is_ascii_hexdigit())
        {
            return PlaintextShape::HexColonHex;
        }
    }
    PlaintextShape::Binary
}

pub fn probe_failure_hint(probe: &SessionKeysDecryptProbe) -> String {
    if probe.chain_keys.is_empty() {
        return "no secure/hello cert chain available for comparison".into();
    }
    if probe
        .chain_keys
        .iter()
        .any(|key| key.matches_audioserver_pubkey)
    {
        return "hello chain contains audioserver SPKI but decrypt still failed; try alternate RSA layouts or check encryption_type".into();
    }
    "none of hello chain SPKIs match audioserver pubkey; Miniserver likely encrypts with Config-stored audioserver key".into()
}

pub fn log_session_keys_probe(probe: &SessionKeysDecryptProbe) {
    if let Some(keys) = &probe.decrypted {
        let shape = probe
            .plaintext_shape
            .unwrap_or(classify_plaintext_shape(&keys.raw));
        debug!(
            len = keys.raw.len(),
            scheme = keys.scheme.label(),
            plaintext_shape = shape.label(),
            "session_keys decrypted"
        );
        return;
    }

    let hint = probe_failure_hint(probe);
    debug!(
        audioserver_pubkey_fp = %probe.audioserver_pubkey_fp,
        encryption_type = probe.encryption_type,
        hello_chain_certs = probe.chain_keys.len(),
        jwt_signer_index = ?probe.jwt_signer_index,
        hint = %hint,
        "session_keys decrypt failed (non-fatal for Config onboarding)"
    );
    for key in &probe.chain_keys {
        debug!(
            chain_index = key.index,
            subject_cn = %key.subject_cn,
            spki_fp = %key.spki_fp,
            rsa_bits = key.rsa_bits,
            jwt_signer = key.verifies_init_jwt,
            matches_audioserver = key.matches_audioserver_pubkey,
            "hello cert chain key"
        );
    }

    if is_probe_session_keys_decrypt() {
        for attempt in &probe.attempts {
            debug!(
                layout = attempt.layout.label(),
                scheme = attempt.scheme.label(),
                scheme_block1 = ?attempt.scheme_block1,
                success = attempt.success,
                "session_keys decrypt probe attempt"
            );
        }
        if let Some(error) = &probe.primary_error {
            debug!(error = %error, "session_keys decrypt primary error");
        }
    }
}

pub fn probe_session_keys_decrypt(
    ciphertext_b64: &str,
    private_key: &RsaPrivateKey,
    public_key: &RsaPublicKey,
    hello: Option<&SecureHelloState>,
    jwt: &str,
    encryption_type: u32,
) -> SessionKeysDecryptProbe {
    let audioserver_pubkey_fp =
        public_key_fingerprint(public_key).unwrap_or_else(|_| "unknown".into());

    let (chain_keys, jwt_signer_index) = match hello {
        Some(state) => match parse_cert_chain_pem(&state.cert_chain_b64) {
            Ok(chain) => {
                let jwt_signer_index = find_verify_key_index_in_chain(&chain, jwt);
                let summary =
                    summarize_hello_cert_chain(&chain, jwt, public_key).unwrap_or_default();
                (summary, jwt_signer_index)
            }
            Err(_) => (Vec::new(), None),
        },
        None => (Vec::new(), None),
    };

    let mut attempts = Vec::new();

    let mut primary_error = match decrypt_session_keys(ciphertext_b64, private_key, encryption_type)
    {
        Ok(keys) => {
            let plaintext_shape = classify_plaintext_shape(&keys.raw);
            return SessionKeysDecryptProbe {
                audioserver_pubkey_fp,
                encryption_type,
                chain_keys,
                jwt_signer_index,
                decrypted: Some(keys),
                plaintext_shape: Some(plaintext_shape),
                attempts,
                primary_error: None,
            };
        }
        Err(SecureInitError::DecryptFailed(msg)) => Some(msg),
        Err(error) => {
            return SessionKeysDecryptProbe {
                audioserver_pubkey_fp,
                encryption_type,
                chain_keys,
                jwt_signer_index,
                decrypted: None,
                plaintext_shape: None,
                attempts,
                primary_error: Some(error.to_string()),
            };
        }
    };

    let cipher = match BASE64_STANDARD.decode(ciphertext_b64.trim()) {
        Ok(bytes) => bytes,
        Err(error) => {
            primary_error = Some(error.to_string());
            return SessionKeysDecryptProbe {
                audioserver_pubkey_fp,
                encryption_type,
                chain_keys,
                jwt_signer_index,
                decrypted: None,
                plaintext_shape: None,
                attempts,
                primary_error,
            };
        }
    };

    if cipher.len() == SESSION_KEYS_CIPHER_LEN {
        let blocks = [&cipher[..RSA_CIPHERTEXT_LEN], &cipher[RSA_CIPHERTEXT_LEN..]];
        let schemes = schemes_for_encryption_type(encryption_type);
        let swapped: [&[u8]; 2] = [blocks[1], blocks[0]];

        for scheme in &schemes {
            let success = decrypt_single_block(private_key, blocks[0], *scheme).is_ok();
            attempts.push(DecryptAttempt {
                layout: DecryptLayout::SingleBlock,
                scheme: *scheme,
                scheme_block1: None,
                success,
            });
            if success {
                if let Ok(raw) = decrypt_single_block(private_key, blocks[0], *scheme) {
                    let plaintext_shape = classify_plaintext_shape(&raw);
                    return SessionKeysDecryptProbe {
                        audioserver_pubkey_fp,
                        encryption_type,
                        chain_keys,
                        jwt_signer_index,
                        decrypted: Some(DecryptedSessionKeys {
                            raw,
                            scheme: *scheme,
                        }),
                        plaintext_shape: Some(plaintext_shape),
                        attempts,
                        primary_error,
                    };
                }
            }
        }

        for scheme in &schemes {
            let success = decrypt_blocks_mixed(private_key, &swapped, [*scheme, *scheme]).is_ok();
            attempts.push(DecryptAttempt {
                layout: DecryptLayout::SwappedBlocks,
                scheme: *scheme,
                scheme_block1: Some(*scheme),
                success,
            });
            if success {
                if let Ok(raw) = decrypt_blocks_mixed(private_key, &swapped, [*scheme, *scheme]) {
                    let plaintext_shape = classify_plaintext_shape(&raw);
                    return SessionKeysDecryptProbe {
                        audioserver_pubkey_fp,
                        encryption_type,
                        chain_keys,
                        jwt_signer_index,
                        decrypted: Some(DecryptedSessionKeys {
                            raw,
                            scheme: *scheme,
                        }),
                        plaintext_shape: Some(plaintext_shape),
                        attempts,
                        primary_error,
                    };
                }
            }
        }

        for scheme0 in DecryptScheme::all() {
            for scheme1 in DecryptScheme::all() {
                let success =
                    decrypt_blocks_mixed(private_key, &swapped, [scheme0, scheme1]).is_ok();
                if is_probe_session_keys_decrypt() || success {
                    attempts.push(DecryptAttempt {
                        layout: DecryptLayout::SwappedBlocks,
                        scheme: scheme1,
                        scheme_block1: Some(scheme0),
                        success,
                    });
                }
                if success {
                    if let Ok(raw) = decrypt_blocks_mixed(private_key, &swapped, [scheme0, scheme1])
                    {
                        let plaintext_shape = classify_plaintext_shape(&raw);
                        return SessionKeysDecryptProbe {
                            audioserver_pubkey_fp,
                            encryption_type,
                            chain_keys,
                            jwt_signer_index,
                            decrypted: Some(DecryptedSessionKeys {
                                raw,
                                scheme: scheme0,
                            }),
                            plaintext_shape: Some(plaintext_shape),
                            attempts,
                            primary_error,
                        };
                    }
                }
            }
        }
    }

    SessionKeysDecryptProbe {
        audioserver_pubkey_fp,
        encryption_type,
        chain_keys,
        jwt_signer_index,
        decrypted: None,
        plaintext_shape: None,
        attempts,
        primary_error,
    }
}

fn system_time_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64
}

/// Unsigned JWT for unit/integration tests (parse + validate only; signature not checked).
#[cfg(test)]
pub fn test_unsigned_init_jwt(session_token: &str, now: SystemTime) -> String {
    let iat = system_time_secs(now);
    let payload = SecureInitRequest {
        sub: INIT_SUB.to_string(),
        session_token: session_token.to_string(),
        session_keys: BASE64_STANDARD.encode([0u8; 512]),
        encryption_type: 1,
        iat,
        exp: iat + 1800,
    };
    let payload_b64 = BASE64_URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("e30.{payload_b64}.e30")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{load_keys, DEFAULT_REMOTE_SESSION_TOKEN};
    use std::fs;

    fn load_fixture() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/secure_init_7095.json"
        );
        let raw = fs::read_to_string(path).expect("fixture");
        serde_json::from_str(&raw).expect("fixture json")
    }

    fn fixture_state_with_hello(fixture: &serde_json::Value) -> SecureSessionState {
        SecureSessionState {
            expected_session_token: fixture["greeting_session_token"]
                .as_str()
                .unwrap()
                .to_string(),
            hello: Some(SecureHelloState {
                client_time: "1779215299".into(),
                cert_chain_b64: fixture["hello_cert_chain_b64"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            }),
            last_init: None,
            decrypted_keys: None,
        }
    }

    #[test]
    fn parse_audit_init_jwt() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let expected = &fixture["expected_request"];
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        assert_eq!(req.sub, expected["sub"].as_str().unwrap());
        assert_eq!(
            req.session_token,
            expected["session_token"].as_str().unwrap()
        );
        assert_eq!(
            req.encryption_type,
            expected["encryption_type"].as_u64().unwrap() as u32
        );
        assert_eq!(req.iat, expected["iat"].as_i64().unwrap());
        assert_eq!(req.exp, expected["exp"].as_i64().unwrap());
    }

    #[test]
    fn parse_hello_cert_chain_from_trace() {
        let fixture = load_fixture();
        let chain_b64 = fixture["hello_cert_chain_b64"].as_str().unwrap();
        let chain = parse_cert_chain_pem(chain_b64).unwrap();
        assert!(!chain.is_empty());
        assert!(miniserver_verify_key(&chain).is_ok());
    }

    #[test]
    fn verify_audit_init_jwt() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let state = fixture_state_with_hello(&fixture);
        let chain = parse_cert_chain_pem(&state.hello.as_ref().unwrap().cert_chain_b64).unwrap();
        let key = find_verify_key_for_jwt(&chain, jwt)
            .expect("fixture JWT must verify against hello cert chain");
        let req = verify_init_jwt(jwt, &key).unwrap();
        assert_eq!(req.sub, INIT_SUB);
        assert_eq!(req.session_token, DEFAULT_REMOTE_SESSION_TOKEN);
    }

    #[test]
    fn verify_rejects_tampered_payload() {
        let fixture = load_fixture();
        let mut jwt = fixture["init_jwt"].as_str().unwrap().to_string();
        let parts: Vec<&str> = jwt.split('.').collect();
        let mut payload = BASE64_URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
        if !payload.is_empty() {
            payload[0] ^= 0x01;
        }
        let tampered_payload = BASE64_URL_SAFE_NO_PAD.encode(&payload);
        jwt = format!("{}.{}.{}", parts[0], tampered_payload, parts[2]);

        let state = fixture_state_with_hello(&fixture);
        let chain = parse_cert_chain_pem(&state.hello.as_ref().unwrap().cert_chain_b64).unwrap();
        let key = miniserver_verify_key(&chain).unwrap();
        assert!(verify_init_jwt(&jwt, &key).is_err());
    }

    #[test]
    fn session_keys_is_512_bytes() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let decoded = BASE64_STANDARD
            .decode(&req.session_keys)
            .expect("session_keys base64");
        assert_eq!(decoded.len(), 512);
    }

    #[test]
    fn validate_matching_token() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(req.iat as u64 + 1);
        validate_init_request(&req, DEFAULT_REMOTE_SESSION_TOKEN, now).unwrap();
    }

    #[test]
    fn reject_token_mismatch() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(req.iat as u64 + 1);
        let err = validate_init_request(&req, "wrong-token", now).unwrap_err();
        assert_eq!(err, SecureInitError::TokenMismatch);
    }

    #[test]
    fn reject_wrong_sub() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let mut req = parse_jwt_payload_unverified(jwt).unwrap();
        req.sub = "other".to_string();
        let now = UNIX_EPOCH + Duration::from_secs(req.iat as u64 + 1);
        let err = validate_init_request(&req, DEFAULT_REMOTE_SESSION_TOKEN, now).unwrap_err();
        assert!(matches!(err, SecureInitError::WrongSub { .. }));
    }

    #[test]
    fn reject_expired() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs((req.exp + EXP_GRACE_SECS + 1) as u64);
        let err = validate_init_request(&req, DEFAULT_REMOTE_SESSION_TOKEN, now).unwrap_err();
        assert!(matches!(err, SecureInitError::Expired { .. }));
    }

    #[test]
    fn build_success_claims_echoes_token() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(req.iat as u64);
        let success = build_success_claims(&req, now);
        assert_eq!(success.sub, INIT_SUCCESS_SUB);
        assert_eq!(success.session_token, req.session_token);
        assert_eq!(success.iat, req.iat);
        assert_eq!(success.exp, req.iat + SUCCESS_TTL_SECS);
    }

    #[test]
    fn init_response_jwt_is_legacy_stub() {
        let fixture = load_fixture();
        assert_eq!(
            legacy_success_jwt(),
            fixture["expected_response_jwt"].as_str().unwrap()
        );
    }

    #[test]
    fn sign_success_jwt_roundtrip() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let success = build_success_claims(&req, now);
        let (private_key, public_key) = load_keys().unwrap();
        let token = sign_success_jwt(&success, &private_key).unwrap();
        let decoded = verify_success_jwt(&token, &public_key).unwrap();
        assert_eq!(decoded.sub, INIT_SUCCESS_SUB);
        assert_eq!(decoded.session_token, req.session_token);
        assert_eq!(decoded.iat, success.iat);
        assert_eq!(decoded.exp, success.exp);
    }

    #[test]
    fn decrypt_session_keys_roundtrip_pkcs1() {
        roundtrip_decrypt_scheme(DecryptScheme::Pkcs1v15);
    }

    #[test]
    fn decrypt_session_keys_roundtrip_oaep_sha1() {
        roundtrip_decrypt_scheme(DecryptScheme::OaepSha1);
    }

    #[test]
    fn decrypt_session_keys_roundtrip_oaep_sha256() {
        roundtrip_decrypt_scheme(DecryptScheme::OaepSha256);
    }

    fn roundtrip_decrypt_scheme(scheme: DecryptScheme) {
        use rsa::PrivateKey as RsaPrivateKey;

        let mut rng = rand::rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let public_key = private_key.to_public_key();

        let mut cipher = Vec::with_capacity(512);
        // Session-key shape (`hex:hex`) split over the two RSA blocks.
        let chunks: [&[u8]; 2] = [
            b"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
            b":0102030405060708090a0b0c0d0e0f10",
        ];
        for chunk in chunks {
            let encrypted = match scheme {
                DecryptScheme::Pkcs1v15 => public_key
                    .encrypt(&mut rng, Pkcs1v15Encrypt, chunk)
                    .unwrap(),
                DecryptScheme::OaepSha1 => public_key
                    .encrypt(&mut rng, Oaep::<sha1::Sha1>::new(), chunk)
                    .unwrap(),
                DecryptScheme::OaepSha256 => public_key
                    .encrypt(&mut rng, Oaep::<sha2::Sha256>::new(), chunk)
                    .unwrap(),
            };
            assert_eq!(encrypted.len(), RSA_CIPHERTEXT_LEN);
            cipher.extend_from_slice(&encrypted);
        }

        let b64 = BASE64_STANDARD.encode(&cipher);
        let keys = decrypt_session_keys(&b64, &private_key, 1).unwrap();
        assert_eq!(keys.scheme, scheme);
        assert!(keys.raw.len() >= 32);
    }

    #[test]
    fn pkcs1v15_garbage_is_not_taken_as_session_keys() {
        use rsa::PrivateKey as RsaPrivateKey;

        let mut rng = rand::rng();
        let right = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let wrong = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let mut cipher = Vec::new();
        for chunk in [&b"0011aabb"[..], &b":ccdd"[..]] {
            let block = right
                .to_public_key()
                .encrypt(&mut rng, Pkcs1v15Encrypt, chunk)
                .unwrap();
            cipher.extend_from_slice(&block);
        }
        let b64 = BASE64_STANDARD.encode(&cipher);
        // Implicit rejection hands the wrong key synthetic plaintext instead of an error.
        assert!(decrypt_session_keys(&b64, &wrong, 1).is_err());
        let keys = decrypt_session_keys(&b64, &right, 1).unwrap();
        assert_eq!(keys.raw, b"0011aabb:ccdd");
        assert_eq!(keys.scheme, DecryptScheme::Pkcs1v15);
    }

    #[test]
    #[ignore = "requires audioserver private.pem paired with Miniserver session_keys ciphertext"]
    fn decrypt_session_keys_fixture_with_paired_key() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let (private_key, _) = load_keys().unwrap();
        let keys =
            decrypt_session_keys(&req.session_keys, &private_key, req.encryption_type).unwrap();
        assert!(!keys.raw.is_empty());
    }

    #[test]
    fn summarize_hello_cert_chain_from_fixture() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let chain_b64 = fixture["hello_cert_chain_b64"].as_str().unwrap();
        let chain = parse_cert_chain_pem(chain_b64).unwrap();
        let (_, public_key) = load_keys().unwrap();
        let summary = summarize_hello_cert_chain(&chain, jwt, &public_key).unwrap();
        assert!(!summary.is_empty());
        assert_eq!(
            summary.iter().filter(|key| key.verifies_init_jwt).count(),
            1
        );
        assert!(summary.iter().all(|key| !key.matches_audioserver_pubkey));
    }

    #[test]
    fn classify_plaintext_shape_hex_colon_hex() {
        assert_eq!(
            classify_plaintext_shape(b"0123abcd:fedcba98"),
            PlaintextShape::HexColonHex
        );
        assert_eq!(classify_plaintext_shape(b""), PlaintextShape::Empty);
        assert_eq!(classify_plaintext_shape(b"not-hex"), PlaintextShape::Binary);
    }

    #[test]
    fn probe_reports_audioserver_fp_on_failure() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let state = fixture_state_with_hello(&fixture);
        let (private_key, public_key) = load_keys().unwrap();
        let probe = probe_session_keys_decrypt(
            &req.session_keys,
            &private_key,
            &public_key,
            state.hello.as_ref(),
            jwt,
            req.encryption_type,
        );
        assert!(probe.decrypted.is_none());
        assert!(!probe.audioserver_pubkey_fp.is_empty());
        assert!(!probe.chain_keys.is_empty());
        assert!(probe.jwt_signer_index.is_some());
        assert!(probe.primary_error.is_some());
        assert!(probe_failure_hint(&probe).contains("Config-stored audioserver key"));
    }

    #[test]
    fn parse_jwt_rejects_malformed_token() {
        assert!(matches!(
            parse_jwt_payload_unverified("only-one-part"),
            Err(SecureInitError::InvalidJwtFormat)
        ));
        assert!(matches!(
            parse_jwt_payload_unverified("a.b.c.d"),
            Err(SecureInitError::InvalidJwtFormat)
        ));
    }

    #[test]
    fn parse_jwt_rejects_invalid_payload_base64() {
        assert!(matches!(
            parse_jwt_payload_unverified("e30.!!!.e30"),
            Err(SecureInitError::InvalidJwtPayload(_))
        ));
    }

    #[test]
    fn resolve_init_without_hello_uses_unverified_parse() {
        let token = test_unsigned_init_jwt(DEFAULT_REMOTE_SESSION_TOKEN, UNIX_EPOCH);
        let state = SecureSessionState::default();
        let req = resolve_init_request(&token, &state).unwrap();
        assert_eq!(req.sub, INIT_SUB);
        assert_eq!(req.session_token, DEFAULT_REMOTE_SESSION_TOKEN);
    }

    #[test]
    fn validate_accepts_jwt_within_grace_period() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs((req.exp + EXP_GRACE_SECS - 1) as u64);
        validate_init_request(&req, DEFAULT_REMOTE_SESSION_TOKEN, now).unwrap();
    }

    #[test]
    fn validate_rejects_jwt_past_grace_period() {
        let fixture = load_fixture();
        let jwt = fixture["init_jwt"].as_str().unwrap();
        let req = parse_jwt_payload_unverified(jwt).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs((req.exp + EXP_GRACE_SECS) as u64);
        let err = validate_init_request(&req, DEFAULT_REMOTE_SESSION_TOKEN, now).unwrap_err();
        assert!(matches!(err, SecureInitError::Expired { .. }));
    }
}
