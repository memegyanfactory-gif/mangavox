//! The cloud handshake and sealed messages (CLOUD.md, "Handshake" and "Sealed messages"): ECDH
//! P-256, HKDF-SHA256 and AES-256-GCM, matching what `cloud/web/tunnel.js` does with WebCrypto.
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hkdf::Hkdf;
use p256::ecdh::{diffie_hellman, EphemeralSecret};
use p256::elliptic_curve::sec1::ToEncodedPoint as _;
use p256::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};

pub const KIND_PAIR: u8 = 1;
pub const KIND_RESUME: u8 = 2;
pub const KIND_REPLY: u8 = 3;
pub const KIND_REFUSE: u8 = 4;
pub const KIND_SEALED: u8 = 5;

/// Sealed plaintext kinds.
pub const MSG_JSON: u8 = 0;
pub const MSG_DATA: u8 = 1;
pub const MSG_END: u8 = 2;

const DIR_PHONE_TO_PC: u32 = 1;
const DIR_PC_TO_PHONE: u32 = 2;
const LABEL: &[u8] = b"MangaVox cloud v1";

/// A 65-byte uncompressed SEC1 point.
pub fn point(pk: &PublicKey) -> Vec<u8> {
    pk.to_encoded_point(false).as_bytes().to_vec()
}

/// A public key from base64url of a 65-byte uncompressed point (anything else is refused).
pub fn parse_point(b64: &str) -> Option<PublicKey> {
    let raw = URL_SAFE_NO_PAD.decode(b64.trim()).ok()?;
    (raw.len() == 65 && raw[0] == 4).then(|| PublicKey::from_sec1_bytes(&raw).ok()).flatten()
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// First 16 bytes of SHA-256 of a public key, base64url: what the QR code pins.
pub fn fingerprint(pk: &PublicKey) -> String {
    b64(&Sha256::digest(point(pk))[..16])
}

/// Everything both sides hash into the session keys.
pub struct Transcript<'a> {
    pub kind: u8,
    pub room: &'a [u8],
    pub ep: &'a PublicKey,
    pub ec: &'a PublicKey,
    pub s: &'a PublicKey,
    pub d: &'a PublicKey,
    pub np: &'a [u8],
    pub nc: &'a [u8],
}

impl Transcript<'_> {
    fn hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(LABEL);
        h.update([self.kind]);
        h.update(self.room);
        for p in [self.ep, self.ec, self.s, self.d] {
            h.update(point(p));
        }
        h.update(self.np);
        h.update(self.nc);
        h.finalize().into()
    }

    /// "123 456": shown on the phone and on the PC's Allow prompt, so the owner can tell their
    /// own phone from someone who also saw the QR code (each session's nonces make it unique).
    pub fn verify_code(&self) -> String {
        let h = self.hash();
        let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % 1_000_000;
        format!("{:03} {:03}", n / 1000, n % 1000)
    }
}

/// The two directions' keys from the three ECDH results.
fn derive(ee: &[u8], se: &[u8], de: &[u8], salt: &[u8; 32], t: &Transcript) -> ([u8; 32], [u8; 32]) {
    let ikm = [ee, se, de].concat();
    let hk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut okm = [0u8; 64];
    // 64 bytes is far below HKDF-SHA256's limit.
    let _ = hk.expand(&t.hash(), &mut okm);
    let (a, b) = okm.split_at(32);
    (a.try_into().unwrap_or_default(), b.try_into().unwrap_or_default())
}

/// Salt of a RESUME handshake (a PAIR uses the pairing code).
pub const RESUME_SALT: [u8; 32] = [0; 32];

/// The PC's side: its ephemeral key `ec`, its static key `s`.
pub fn pc_session(ec: &EphemeralSecret, s: &SecretKey, salt: &[u8; 32], t: &Transcript) -> (Opener, Sealer) {
    let ee = ec.diffie_hellman(t.ep);
    let se = diffie_hellman(s.to_nonzero_scalar(), t.ep.as_affine());
    let de = ec.diffie_hellman(t.d);
    let (p2c, c2p) = derive(ee.raw_secret_bytes(), se.raw_secret_bytes(), de.raw_secret_bytes(), salt, t);
    (Opener::new(&p2c, DIR_PHONE_TO_PC), Sealer::new(&c2p, DIR_PC_TO_PHONE))
}

/// The phone's side (tests; the real phone is `tunnel.js`).
#[cfg(test)]
pub fn phone_session(ep: &EphemeralSecret, d: &SecretKey, salt: &[u8; 32], t: &Transcript) -> (Opener, Sealer) {
    let ee = ep.diffie_hellman(t.ec);
    let se = ep.diffie_hellman(t.s);
    let de = diffie_hellman(d.to_nonzero_scalar(), t.ec.as_affine());
    let (p2c, c2p) = derive(ee.raw_secret_bytes(), se.raw_secret_bytes(), de.raw_secret_bytes(), salt, t);
    (Opener::new(&c2p, DIR_PC_TO_PHONE), Sealer::new(&p2c, DIR_PHONE_TO_PC))
}

fn nonce(dir: u32, ctr: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..4].copy_from_slice(&dir.to_be_bytes());
    n[4..].copy_from_slice(&ctr.to_be_bytes());
    n
}

/// Seals outgoing messages; the counter makes every nonce unique.
pub struct Sealer {
    cipher: Aes256Gcm,
    dir: u32,
    ctr: u64,
}

impl Sealer {
    fn new(key: &[u8; 32], dir: u32) -> Self {
        Self { cipher: Aes256Gcm::new(key.into()), dir, ctr: 0 }
    }

    /// A whole SEALED frame (kind byte included).
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let ctr = self.ctr;
        self.ctr += 1;
        let n = nonce(self.dir, ctr);
        // Encrypting into a Vec cannot fail for these sizes.
        let ct = self.cipher.encrypt(Nonce::from_slice(&n), plain).unwrap_or_default();
        let mut out = Vec::with_capacity(9 + ct.len());
        out.push(KIND_SEALED);
        out.extend_from_slice(&ctr.to_be_bytes());
        out.extend_from_slice(&ct);
        out
    }
}

/// Opens incoming messages in order: a counter other than the next one (a replay, a reorder or
/// a drop) or a bad tag fails.
pub struct Opener {
    cipher: Aes256Gcm,
    dir: u32,
    next: u64,
}

impl Opener {
    fn new(key: &[u8; 32], dir: u32) -> Self {
        Self { cipher: Aes256Gcm::new(key.into()), dir, next: 0 }
    }

    /// `frame` is a whole SEALED frame (kind byte included).
    pub fn open(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < 1 + 8 + 16 || frame[0] != KIND_SEALED {
            return None;
        }
        let ctr = u64::from_be_bytes(frame[1..9].try_into().ok()?);
        if ctr != self.next || ctr == u64::MAX {
            return None;
        }
        let plain = self.cipher.decrypt(Nonce::from_slice(&nonce(self.dir, ctr)), &frame[9..]).ok()?;
        self.next += 1;
        Some(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn sessions(salt_pc: [u8; 32], salt_phone: [u8; 32]) -> ((Opener, Sealer), (Opener, Sealer)) {
        let s = SecretKey::random(&mut OsRng);
        let d = SecretKey::random(&mut OsRng);
        let ep = EphemeralSecret::random(&mut OsRng);
        let ec = EphemeralSecret::random(&mut OsRng);
        let (epp, ecp, sp, dp) = (ep.public_key(), ec.public_key(), s.public_key(), d.public_key());
        let t = Transcript { kind: KIND_PAIR, room: b"room", ep: &epp, ec: &ecp, s: &sp, d: &dp, np: b"np", nc: b"nc" };
        (pc_session(&ec, &s, &salt_pc, &t), phone_session(&ep, &d, &salt_phone, &t))
    }

    #[test]
    fn both_sides_agree_and_reject_tampering() {
        let ((mut pc_open, mut pc_seal), (mut ph_open, mut ph_seal)) = sessions([7; 32], [7; 32]);
        let f = ph_seal.seal(b"hello pc");
        assert_eq!(pc_open.open(&f).as_deref(), Some(&b"hello pc"[..]));
        assert!(pc_open.open(&f).is_none(), "a replay fails");
        let f = pc_seal.seal(b"hello phone");
        let mut bad = f.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(ph_open.open(&bad).is_none(), "a flipped bit fails");
        assert_eq!(ph_open.open(&f).as_deref(), Some(&b"hello phone"[..]));
        // Skipping a message (a drop or reorder) fails.
        let _lost = ph_seal.seal(b"1");
        assert!(pc_open.open(&ph_seal.seal(b"2")).is_none());
        // The directions use different keys: a phone frame reflected back fails.
        let ((_, _), (mut ph_open2, mut ph_seal2)) = sessions([1; 32], [1; 32]);
        let own = ph_seal2.seal(b"x");
        assert!(ph_open2.open(&own).is_none());
    }

    #[test]
    fn a_wrong_pairing_code_gives_other_keys() {
        let ((mut pc_open, _), (_, mut ph_seal)) = sessions([7; 32], [8; 32]);
        assert!(pc_open.open(&ph_seal.seal(b"fin")).is_none());
    }

    #[test]
    fn points_parse_strictly() {
        let k = SecretKey::random(&mut OsRng).public_key();
        let enc = b64(&point(&k));
        assert_eq!(parse_point(&enc), Some(k));
        let compressed = b64(k.to_encoded_point(true).as_bytes());
        assert!(parse_point(&compressed).is_none());
        assert!(parse_point("not base64!").is_none());
        assert!(parse_point(&b64(&[4u8; 65])).is_none(), "not on the curve");
        assert_eq!(fingerprint(&k).len(), 22);
    }
}
