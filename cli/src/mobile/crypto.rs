//! Key schedule, handshake proofs and frame sealing for the mobile gateway.
//! Every value here is pinned by the spec's "Test vectors" appendix so the
//! phone apps can check their own implementations byte for byte.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 32;
const KEY_SALT: &[u8] = b"chrome-use mobile v1";
const PHONE_PROOF: &[u8] = b"chrome-use mobile v1 phone proof";
const GATEWAY_PROOF: &[u8] = b"chrome-use mobile v1 gateway proof";
/// Sequence number prefix carried on every encrypted frame.
const SEQ_LEN: usize = 8;
const TAG_LEN: usize = 16;

/// HKDF-SHA256 (RFC 5869) for one 32-byte output block. An empty salt is
/// the RFC's string of zero bytes.
pub fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; KEY_LEN] {
    let zeros = [0u8; KEY_LEN];
    let salt = if salt.is_empty() { &zeros[..] } else { salt };
    let mut extract = <HmacSha256 as Mac>::new_from_slice(salt).expect("HMAC takes any key length");
    extract.update(ikm);
    let prk = extract.finalize().into_bytes();
    let mut expand = <HmacSha256 as Mac>::new_from_slice(&prk).expect("HMAC takes any key length");
    expand.update(info);
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

/// Long-term keys of one paired device, all derived from its pair secret.
#[derive(Clone)]
pub struct DeviceKeys {
    pub auth: [u8; KEY_LEN],
    pub frame: [u8; KEY_LEN],
    pub push: [u8; KEY_LEN],
}

impl DeviceKeys {
    pub fn derive(pair_secret: &[u8; KEY_LEN]) -> Self {
        let frame = hkdf(pair_secret, KEY_SALT, b"frame");
        Self {
            auth: hkdf(pair_secret, KEY_SALT, b"auth"),
            push: hkdf(&frame, b"", b"push"),
            frame,
        }
    }

    /// Per-connection keys: (gateway-to-phone "c2p", phone-to-gateway "p2c").
    pub fn session(
        &self,
        gateway_nonce: &[u8; NONCE_LEN],
        phone_nonce: &[u8; NONCE_LEN],
    ) -> ([u8; KEY_LEN], [u8; KEY_LEN]) {
        let mut salt = [0u8; NONCE_LEN * 2];
        salt[..NONCE_LEN].copy_from_slice(gateway_nonce);
        salt[NONCE_LEN..].copy_from_slice(phone_nonce);
        (
            hkdf(&self.frame, &salt, b"c2p"),
            hkdf(&self.frame, &salt, b"p2c"),
        )
    }
}

fn proof_mac(
    key: &[u8; KEY_LEN],
    label: &[u8],
    gateway_nonce: &[u8; NONCE_LEN],
    phone_nonce: &[u8; NONCE_LEN],
    device_id: &str,
) -> HmacSha256 {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(label);
    mac.update(gateway_nonce);
    mac.update(phone_nonce);
    mac.update(device_id.as_bytes());
    mac
}

/// The phone's proof of the pair secret (computed by the phone; here for
/// the test vectors). Labels keep the two directions'
/// proofs distinct, so neither side's proof can be reflected back.
#[cfg(test)]
pub fn phone_proof(
    keys: &DeviceKeys,
    gateway_nonce: &[u8; NONCE_LEN],
    phone_nonce: &[u8; NONCE_LEN],
    device_id: &str,
) -> [u8; 32] {
    proof_mac(
        &keys.auth,
        PHONE_PROOF,
        gateway_nonce,
        phone_nonce,
        device_id,
    )
    .finalize()
    .into_bytes()
    .into()
}

pub fn gateway_proof(
    keys: &DeviceKeys,
    gateway_nonce: &[u8; NONCE_LEN],
    phone_nonce: &[u8; NONCE_LEN],
    device_id: &str,
) -> [u8; 32] {
    proof_mac(
        &keys.auth,
        GATEWAY_PROOF,
        gateway_nonce,
        phone_nonce,
        device_id,
    )
    .finalize()
    .into_bytes()
    .into()
}

/// Constant-time check of the phone's proof.
pub fn verify_phone_proof(
    keys: &DeviceKeys,
    gateway_nonce: &[u8; NONCE_LEN],
    phone_nonce: &[u8; NONCE_LEN],
    device_id: &str,
    proof: &[u8],
) -> bool {
    proof_mac(
        &keys.auth,
        PHONE_PROOF,
        gateway_nonce,
        phone_nonce,
        device_id,
    )
    .verify_slice(proof)
    .is_ok()
}

pub fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).map_err(|_| "System randomness is unavailable")?;
    Ok(bytes)
}

pub fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn from_b64url<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
    bytes.try_into().ok()
}

fn gcm_nonce(sequence: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&sequence.to_be_bytes());
    nonce
}

/// One direction of an encrypted connection. A frame is the 8-byte big-endian
/// sequence number followed by AES-256-GCM ciphertext and tag; the GCM nonce
/// is four zero bytes and the same sequence number, and the sequence bytes are
/// also the associated data.
pub struct Channel {
    cipher: Aes256Gcm,
    sequence: u64,
}

impl Channel {
    pub fn new(key: &[u8; KEY_LEN]) -> Self {
        Self {
            cipher: Aes256Gcm::new(key.into()),
            sequence: 0,
        }
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let sequence = self.sequence;
        self.sequence = sequence
            .checked_add(1)
            .ok_or("Connection sequence exhausted; reconnect")?;
        let header = sequence.to_be_bytes();
        let sealed = self
            .cipher
            .encrypt(
                Nonce::from_slice(&gcm_nonce(sequence)),
                Payload {
                    msg: plaintext,
                    aad: &header,
                },
            )
            .map_err(|_| "Frame encryption failed")?;
        let mut frame = Vec::with_capacity(SEQ_LEN + sealed.len());
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&sealed);
        Ok(frame)
    }

    /// Open the next frame. Anything but exactly the next sequence number,
    /// or a frame that fails authentication, is an error and the caller
    /// closes the connection.
    pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>, String> {
        if frame.len() < SEQ_LEN + TAG_LEN {
            return Err("Frame is too short".into());
        }
        let header: [u8; SEQ_LEN] = frame[..SEQ_LEN].try_into().expect("checked length");
        let sequence = u64::from_be_bytes(header);
        if sequence != self.sequence {
            return Err("Frame is out of sequence".into());
        }
        let plaintext = self
            .cipher
            .decrypt(
                Nonce::from_slice(&gcm_nonce(sequence)),
                Payload {
                    msg: &frame[SEQ_LEN..],
                    aad: &header,
                },
            )
            .map_err(|_| "Frame failed authentication")?;
        self.sequence = sequence
            .checked_add(1)
            .ok_or("Connection sequence exhausted; reconnect")?;
        Ok(plaintext)
    }
}

/// Encrypt a push body: standard base64 of nonce(12) || ciphertext || tag,
/// no associated data.
pub fn seal_push(
    push_key: &[u8; KEY_LEN],
    nonce: &[u8; 12],
    plaintext: &[u8],
) -> Result<String, String> {
    let cipher = Aes256Gcm::new(push_key.into());
    let sealed = cipher
        .encrypt(Nonce::from_slice(nonce), plaintext)
        .map_err(|_| "Push encryption failed")?;
    let mut out = Vec::with_capacity(12 + sealed.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&sealed);
    Ok(STANDARD.encode(out))
}

#[cfg(test)]
pub fn open_push(push_key: &[u8; KEY_LEN], payload: &str) -> Result<Vec<u8>, String> {
    let raw = STANDARD.decode(payload).map_err(|_| "bad base64")?;
    if raw.len() < 12 + TAG_LEN {
        return Err("short".into());
    }
    Aes256Gcm::new(push_key.into())
        .decrypt(Nonce::from_slice(&raw[..12]), &raw[12..])
        .map_err(|_| "auth".into())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sequence<const N: usize>(start: u8) -> [u8; N] {
        let mut bytes = [0u8; N];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = start.wrapping_add(i as u8);
        }
        bytes
    }

    pub const DEVICE_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    pub const FRAME_TEXT: &str = r#"{"id":1,"op":"conversationList","params":{}}"#;
    pub const PUSH_TEXT: &str = r#"{"v":1,"conversationId":"7d444840-9dc0-41a2-9fb3-1d5b5d3b6c2e","kind":"needsInput","title":"Weekly report","body":"Waiting for your approval"}"#;

    #[test]
    fn hkdf_matches_rfc5869_case_1() {
        // RFC 5869 A.1, first 32 bytes of OKM.
        let ikm = [0x0bu8; 22];
        let salt = sequence::<13>(0);
        let info = sequence::<10>(0xf0);
        assert_eq!(
            hex::encode(hkdf(&ikm, &salt, &info)),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf"
        );
    }

    #[test]
    fn spec_test_vectors() {
        let secret = sequence::<32>(0x00);
        let gateway = sequence::<32>(0x20);
        let phone = sequence::<32>(0x40);
        let keys = DeviceKeys::derive(&secret);
        assert_eq!(
            hex::encode(keys.auth),
            "a8344e0caf1c7d238af084ccdbeb9d4c2d8ac1abeea4314ca22d1d3aaa47e00e"
        );
        assert_eq!(
            hex::encode(keys.frame),
            "1ab8cb618f09a935128d1aca5e0aa708610fd20cb23a3b473e2446052036400d"
        );
        assert_eq!(
            hex::encode(keys.push),
            "27654b22292a6eab25b4c62370b206e71aac17dfef02e6a2cbf568565f85cd2e"
        );
        let (c2p, p2c) = keys.session(&gateway, &phone);
        assert_eq!(
            hex::encode(c2p),
            "5bc1cde1dbe85a0bafb5c37868e4b8798a5d290b548b2c8eea7831449f33116a"
        );
        assert_eq!(
            hex::encode(p2c),
            "5f2d6790a8083d40200190969fa564fae1390dacab76bd4318c41185f10a6845"
        );
        assert_eq!(
            hex::encode(phone_proof(&keys, &gateway, &phone, DEVICE_ID)),
            "8dc9b0be57af6d6329053ba1807835eb333d617132c1f1c00d494fa21b99ec20"
        );
        assert_eq!(
            hex::encode(gateway_proof(&keys, &gateway, &phone, DEVICE_ID)),
            "7fcae67185c35186b70f2434a8ba5d593d40b2ea074558601cdd76b1e43f34e6"
        );
        let frame = Channel::new(&p2c).seal(FRAME_TEXT.as_bytes()).unwrap();
        assert_eq!(hex::encode(&frame), "0000000000000000eecbbe0b87d98c8799928139b13079008dd2677e13613735094e30ac54db12d587633832f3eda22f16275d8d6cc4cb8d1b21400427884809662cfab2");
        let push = seal_push(&keys.push, &sequence::<12>(0x60), PUSH_TEXT.as_bytes()).unwrap();
        assert_eq!(push, "YGFiY2RlZmdoaWprqoPM7QCXZkhyaRefvHAqHgEY+UOzCuAGhcMb8AMnhTeophCgMirT+P2+EAJ4WLnl48e4jehHaMj98HzDBDhx1zqQUyMs71Fy46iVJiGOKMmmDYeTkc51WH7yIMyUZfp4CADxrSXLZ8BPuy/0RvOBhHcvWPqF/CEQM3EYZ0eNtuz3yRqkiOABYIXUrTn31OrXOL/ukGxSZoSNvQCn4gM=");
        assert_eq!(open_push(&keys.push, &push).unwrap(), PUSH_TEXT.as_bytes());
    }

    #[test]
    fn proofs_verify_and_are_direction_bound() {
        let keys = DeviceKeys::derive(&sequence::<32>(7));
        let (g, p) = (sequence::<32>(1), sequence::<32>(2));
        let proof = phone_proof(&keys, &g, &p, DEVICE_ID);
        assert!(verify_phone_proof(&keys, &g, &p, DEVICE_ID, &proof));
        // The gateway's own proof is not accepted as the phone's.
        let reflected = gateway_proof(&keys, &g, &p, DEVICE_ID);
        assert!(!verify_phone_proof(&keys, &g, &p, DEVICE_ID, &reflected));
        assert!(!verify_phone_proof(&keys, &p, &g, DEVICE_ID, &proof));
        assert!(!verify_phone_proof(&keys, &g, &p, "other", &proof));
        assert!(!verify_phone_proof(&keys, &g, &p, DEVICE_ID, &proof[..31]));
        let other = DeviceKeys::derive(&sequence::<32>(8));
        assert!(!verify_phone_proof(&other, &g, &p, DEVICE_ID, &proof));
    }

    #[test]
    fn channel_rejects_replay_reorder_tamper_and_wrong_key() {
        let key = sequence::<32>(9);
        let mut sender = Channel::new(&key);
        let first = sender.seal(b"one").unwrap();
        let second = sender.seal(b"two").unwrap();
        let mut receiver = Channel::new(&key);
        // Reordered: the second frame first.
        assert!(Channel::new(&key).open(&second).is_err());
        assert_eq!(receiver.open(&first).unwrap(), b"one");
        // Replayed.
        assert!(receiver.open(&first).is_err());
        let mut tampered = second.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(receiver.open(&tampered).is_err());
        // A forged sequence header fails authentication even when expected.
        let mut forged = first.clone();
        forged[7] = 1;
        assert!(receiver.open(&forged).is_err());
        assert_eq!(receiver.open(&second).unwrap(), b"two");
        let mut wrong = Channel::new(&sequence::<32>(10));
        assert!(wrong.open(&first).is_err());
        assert!(receiver.open(&first[..10]).is_err());
    }
}
