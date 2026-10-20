//! Release signatures: each GitHub release carries `SHA256SUMS` (the SHA-256 of every installer,
//! `sha256sum`'s format) and `SHA256SUMS.sig`, an ed25519 signature over the release's version and
//! those sums, made with a key kept off the repository (`cargo xtask sign-release`). The public
//! keys are compiled in (`release_keys.txt`), so a release swapped on GitHub — installer and sums
//! together — doesn't verify, and the version in the signed text stops an old release's files
//! being served as a newer one. Shared by the updater and xtask (`#[path]`).

#![allow(dead_code)]

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

/// The release assets.
pub const SUMS: &str = "SHA256SUMS";
pub const SIG: &str = "SHA256SUMS.sig";
/// The signature file's first line.
const HEADER: &str = "CADaddy release signature 1";
/// Larger sums or signature files are refused.
pub const MAX_FILE: usize = 64 << 10;

/// The public keys releases are signed with (hex, one per line; `#` comments).
const KEYS: &str = include_str!("release_keys.txt");

/// The compiled-in public keys.
pub fn trusted_keys() -> Vec<[u8; 32]> {
    keys_in(KEYS)
}

/// The keys in a keys file's text: 64 hex digits per line, anything after `#` ignored.
pub fn keys_in(text: &str) -> Vec<[u8; 32]> {
    text.lines().filter_map(|l| l.split('#').next().and_then(|k| hex32(k.trim()))).collect()
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect()
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    hex_bytes(s)?.try_into().ok()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// A version as compared: no leading `v`, trimmed.
fn plain(version: &str) -> &str {
    version.trim().trim_start_matches(['v', 'V'])
}

/// What is signed: the version, then the sums file as published.
pub fn message(version: &str, sums: &[u8]) -> Vec<u8> {
    let mut m = format!("CADaddy release {}\n", plain(version)).into_bytes();
    m.extend_from_slice(sums);
    m
}

/// A key's short name in the signature file: the first 8 bytes of its SHA-256, in hex.
pub fn key_id(key: &[u8; 32]) -> String {
    hex(Sha256::digest(key).get(..8).unwrap_or_default())
}

/// The signature file for `sig` made by `key` over release `version`.
pub fn sig_file(version: &str, key: &[u8; 32], sig: &[u8; 64]) -> String {
    format!("{HEADER}\nversion {}\nkey {}\nsig {}\n", plain(version), key_id(key), hex(sig))
}

/// Check `sig_text` (a [`SIG`] file) over `sums` for release `version` against `keys`.
pub fn verify(version: &str, sums: &[u8], sig_text: &str, keys: &[[u8; 32]]) -> Result<(), String> {
    if keys.is_empty() {
        return Err("this build has no release key to check updates with".into());
    }
    if sums.len() > MAX_FILE || sig_text.len() > MAX_FILE {
        return Err("the release's signature files are too large".into());
    }
    let mut lines = sig_text.lines().map(str::trim);
    if lines.next() != Some(HEADER) {
        return Err("the release's signature isn't one CADaddy reads".into());
    }
    let (mut signed_version, mut id, mut sig) = (None, None, None);
    for l in lines {
        match l.split_once(' ') {
            Some(("version", v)) => signed_version = Some(v.trim()),
            Some(("key", k)) => id = Some(k.trim()),
            Some(("sig", s)) => sig = hex_bytes(s.trim()).and_then(|b| <[u8; 64]>::try_from(b).ok()),
            _ => {}
        }
    }
    let sig = sig.ok_or("the release's signature is missing or damaged")?;
    if signed_version != Some(plain(version)) {
        return Err(format!("the release's signature is for another version ({})", signed_version.unwrap_or("none")));
    }
    let id = id.unwrap_or_default();
    let key = keys.iter().find(|k| key_id(k) == id).ok_or("the release is signed with a key this build doesn't trust")?;
    let vk = VerifyingKey::from_bytes(key).map_err(|_| "this build's release key is damaged".to_string())?;
    vk.verify_strict(&message(version, sums), &Signature::from_bytes(&sig)).map_err(|_| "the release's signature doesn't match its files".to_string())
}

/// The SHA-256 (lower-case hex) `sums` lists for the file `name`.
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        // `sha256sum` marks binary mode with `*`.
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed(seed: u8, version: &str, sums: &str) -> ([u8; 32], String) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let pk = sk.verifying_key().to_bytes();
        let sig = sk.sign(&message(version, sums.as_bytes())).to_bytes();
        (pk, sig_file(version, &pk, &sig))
    }

    const SUMS_TEXT: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  CADaddy-Mac.dmg\n\
                             486ea46224d1bb4fb680f34f7c9ad96a8f24ec88be73ea8e5a6c65260e9cb8a7 *CADaddy-Windows.msi\n";

    #[test]
    fn a_signed_release_verifies_and_lists_its_installers() {
        let (pk, sig) = signed(7, "v0.9.0", SUMS_TEXT);
        assert_eq!(verify("0.9.0", SUMS_TEXT.as_bytes(), &sig, &[pk]), Ok(()));
        assert_eq!(sum_for(SUMS_TEXT, "CADaddy-Mac.dmg").as_deref(), Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"));
        assert!(sum_for(SUMS_TEXT, "CADaddy-Windows.msi").is_some(), "binary-mode lines");
        assert_eq!(sum_for(SUMS_TEXT, "CADaddy-Linux.AppImage"), None);
    }

    #[test]
    fn tampered_sums_wrong_key_other_version_or_no_signature_are_refused() {
        let (pk, sig) = signed(7, "0.9.0", SUMS_TEXT);
        let tampered = SUMS_TEXT.replacen("2cf2", "0000", 1);
        assert!(verify("0.9.0", tampered.as_bytes(), &sig, &[pk]).unwrap_err().contains("doesn't match"));
        let (other, _) = signed(9, "0.9.0", SUMS_TEXT);
        assert!(verify("0.9.0", SUMS_TEXT.as_bytes(), &sig, &[other]).unwrap_err().contains("doesn't trust"));
        // An old release's signed files served as a newer release.
        assert!(verify("0.9.1", SUMS_TEXT.as_bytes(), &sig, &[pk]).unwrap_err().contains("another version"));
        // Same key id but a forged signature.
        let forged = sig.replace(&sig[sig.len() - 10..sig.len() - 1], "000000000");
        assert!(verify("0.9.0", SUMS_TEXT.as_bytes(), &forged, &[pk]).is_err());
        for bad in ["", "hello", HEADER, &format!("{HEADER}\nversion 0.9.0\nkey 00\nsig zz\n")] {
            assert!(verify("0.9.0", SUMS_TEXT.as_bytes(), bad, &[pk]).is_err(), "{bad:?}");
        }
        assert!(verify("0.9.0", SUMS_TEXT.as_bytes(), &sig, &[]).unwrap_err().contains("no release key"));
    }

    #[test]
    fn the_keys_file_is_read_and_this_build_has_a_key() {
        let k = format!("# comment\n\n{}  # the 2026 key\nnot a key\n", "ab".repeat(32));
        assert_eq!(keys_in(&k), vec![[0xab; 32]]);
        assert!(!trusted_keys().is_empty(), "release_keys.txt must hold the release key");
        for k in trusted_keys() {
            assert!(VerifyingKey::from_bytes(&k).is_ok(), "a valid ed25519 public key");
        }
    }
}
