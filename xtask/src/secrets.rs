//! A secrets scan of the tree (`cargo xtask secrets`, and part of `cargo xtask supplychain`): the
//! shapes of real credentials — cloud, source host, AI service and payment API keys, chat tokens
//! and private keys — in every file git would commit (tracked, or new and not ignored). No external
//! service: the patterns are here.
//!
//! A deliberate example (a documented placeholder) goes in `.secrets-allow` as `path` or
//! `path:line`, with a comment saying why.

use std::path::Path;
use std::process::Command;

/// A credential shape: its name, a literal prefix, the characters after it and how many.
struct Shape {
    name: &'static str,
    prefix: &'static str,
    body: fn(u8) -> bool,
    min: usize,
    max: usize,
}

fn alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}
fn upper_digit(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}
fn token(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

const SHAPES: &[Shape] = &[
    Shape { name: "AWS access key", prefix: "AKIA", body: upper_digit, min: 16, max: 16 },
    Shape { name: "AWS access key", prefix: "ASIA", body: upper_digit, min: 16, max: 16 },
    Shape { name: "GitHub token", prefix: "ghp_", body: alnum, min: 36, max: 36 },
    Shape { name: "GitHub token", prefix: "gho_", body: alnum, min: 36, max: 36 },
    Shape { name: "GitHub token", prefix: "ghs_", body: alnum, min: 36, max: 36 },
    Shape { name: "GitHub token", prefix: "ghu_", body: alnum, min: 36, max: 36 },
    Shape { name: "GitHub token", prefix: "ghr_", body: alnum, min: 36, max: 36 },
    Shape { name: "GitHub token", prefix: "github_pat_", body: token, min: 70, max: 100 },
    Shape { name: "Anthropic API key", prefix: "sk-ant-", body: token, min: 32, max: 200 },
    Shape { name: "OpenAI API key", prefix: "sk-proj-", body: token, min: 32, max: 200 },
    Shape { name: "xAI API key", prefix: "xai-", body: alnum, min: 40, max: 100 },
    Shape { name: "Google API key", prefix: "AIza", body: token, min: 35, max: 35 },
    Shape { name: "Slack token", prefix: "xoxb-", body: token, min: 20, max: 200 },
    Shape { name: "Slack token", prefix: "xoxp-", body: token, min: 20, max: 200 },
    Shape { name: "Stripe secret key", prefix: "sk_live_", body: alnum, min: 24, max: 120 },
    Shape { name: "Stripe secret key", prefix: "rk_live_", body: alnum, min: 24, max: 120 },
    Shape { name: "npm token", prefix: "npm_", body: alnum, min: 36, max: 36 },
];

/// A placeholder made of one repeated character or `x`/`X`/`0` runs isn't a credential.
fn placeholder(body: &[u8]) -> bool {
    body.iter().all(|c| matches!(c, b'x' | b'X' | b'0' | b'*' | b'.' | b'_' | b'-')) || body.windows(2).all(|w| w[0] == w[1])
}

/// The credentials in one text: (line number, what).
pub fn find(text: &str) -> Vec<(usize, &'static str)> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let bytes = line.as_bytes();
        for shape in SHAPES {
            let mut from = 0;
            while let Some(i) = line.get(from..).and_then(|r| r.find(shape.prefix)).map(|i| i + from) {
                from = i + shape.prefix.len();
                // Not inside a longer word (`MYAKIA…`).
                if i > 0 && bytes.get(i - 1).is_some_and(|c| token(*c)) {
                    continue;
                }
                let body: Vec<u8> = bytes.get(from..).unwrap_or_default().iter().copied().take_while(|c| (shape.body)(*c)).collect();
                // Exactly as long as the shape: no more of the same characters right after.
                let len = body.len();
                if (shape.min..=shape.max).contains(&len) && !placeholder(&body) {
                    out.push((n + 1, shape.name));
                }
            }
        }
        // A private key block (PEM or OpenSSH): `-----BEGIN <CAPITALS> PRIVATE KEY-----`.
        if pem_private_key(line) {
            out.push((n + 1, "private key"));
        }
    }
    out
}

fn pem_private_key(line: &str) -> bool {
    let mut rest = line;
    while let Some(i) = rest.find("-----BEGIN ") {
        let after = rest.get(i + 11..).unwrap_or_default();
        if let Some(end) = after.find("-----") {
            let label = after.get(..end).unwrap_or_default();
            if label.ends_with("PRIVATE KEY") && label.bytes().all(|c| c.is_ascii_uppercase() || c == b' ') {
                return true;
            }
        }
        rest = after;
    }
    false
}

/// The files git would commit: tracked, and new ones not ignored.
fn files(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect())
}

/// Every credential in the tree, as `path:line: what`, less what `.secrets-allow` lists.
pub fn scan(root: &Path) -> Result<Vec<String>, String> {
    let allow = std::fs::read_to_string(root.join(".secrets-allow")).unwrap_or_default();
    let allowed: Vec<&str> = allow.lines().map(|l| l.split('#').next().unwrap_or_default().trim()).filter(|l| !l.is_empty()).collect();
    let mut found = Vec::new();
    for f in files(root)? {
        let path = root.join(&f);
        // Text files up to 8 MB; binaries (a NUL early on) aren't read as text.
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if !meta.is_file() || meta.len() > 8 << 20 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes.iter().take(8000).any(|b| *b == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (line, what) in find(&text) {
            let at = format!("{f}:{line}");
            if !allowed.iter().any(|a| *a == f || *a == at) {
                found.push(format!("{at}: {what}"));
            }
        }
    }
    Ok(found)
}

/// `cargo xtask secrets`.
pub fn run(root: &Path) -> Result<(), String> {
    let found = scan(root)?;
    if found.is_empty() {
        println!("secrets: none found");
        Ok(())
    } else {
        Err(format!(
            "secrets: {} found (remove them and rotate the credentials; a deliberate example goes in .secrets-allow):\n  {}",
            found.len(),
            found.join("\n  ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A credential made at run time, so this file never holds one.
    fn fake(prefix: &str, body: &str, n: usize) -> String {
        format!("{prefix}{}", body.repeat(n.div_ceil(body.len())).get(..n).unwrap_or_default())
    }

    #[test]
    fn credentials_are_found() {
        let cases = [
            (fake("AK", "IA", 2) + &fake("", "Q7ZK3M", 16), "AWS access key"),
            (fake("gh", "p_", 2) + &fake("", "aB3dE5fG7h", 36), "GitHub token"),
            (fake("sk-", "ant-", 4) + &fake("api03-", "Zq9_xY-12", 60), "Anthropic API key"),
            (fake("xa", "i-", 2) + &fake("", "Qw3Er5Ty7U", 60), "xAI API key"),
            (fake("AI", "za", 2) + &fake("", "Sy9-Bc_1De", 35), "Google API key"),
            (fake("sk_", "live_", 5) + &fake("", "4eC39HqLyjWDarjtT1zdp7dc", 24), "Stripe secret key"),
        ];
        for (secret, what) in &cases {
            let text = format!("let key = \"{secret}\";\n");
            assert_eq!(find(&text), vec![(1, *what)], "{secret}");
        }
        let pem = format!("-----BEGIN {}PRIVATE KEY-----\n", "RSA ");
        assert_eq!(find(&pem), vec![(1, "private key")]);
    }

    #[test]
    fn look_alikes_are_not() {
        let not = [
            // Too short, too long, inside a word, a placeholder.
            fake("AK", "IA", 2) + "ABC123",
            fake("AK", "IA", 2) + &fake("", "Q7ZK3M", 20),
            "MY".to_string() + &fake("AK", "IA", 2) + &fake("", "Q7ZK3M", 16),
            fake("sk-", "ant-", 4) + &"x".repeat(40),
            fake("gh", "p_", 2) + &"X".repeat(36),
            "the prefix ghp_ alone in prose".to_string(),
            "-----BEGIN PUBLIC KEY-----".to_string(),
        ];
        for text in &not {
            assert!(find(text).is_empty(), "{text}: {:?}", find(text));
        }
    }

    #[test]
    fn the_tree_has_no_secrets() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(scan(root).unwrap(), Vec::<String>::new());
    }
}
