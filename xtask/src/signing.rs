//! `cargo xtask signing [--require]`: what a release built now would be signed with, per platform,
//! from the signing material in the environment (the names the packaging scripts read; see
//! docs/code-signing.md). Without `--require` it reports and succeeds: test builds are unsigned on
//! purpose. With it, anything that would ship unsigned or unnotarized is an error, so a release run
//! stops before it publishes: one switch once the certificates exist.

/// One platform's signing, as the scripts would do it.
#[derive(Debug, PartialEq)]
pub struct Plan {
    pub platform: &'static str,
    /// Fully signed (and notarized where that applies).
    pub ready: bool,
    pub how: String,
    /// What's missing for `ready`.
    pub missing: Vec<&'static str>,
}

/// The plans for the environment `env` gives (a variable set and not empty).
pub fn plans(env: &dyn Fn(&str) -> Option<String>) -> Vec<Plan> {
    let set = |k: &str| env(k).is_some_and(|v| !v.trim().is_empty());
    let mut out = Vec::new();

    // Windows (packaging/windows/sign.ps1): a .pfx, or Azure Trusted Signing.
    const AZURE: [&str; 6] =
        ["AZURE_TENANT_ID", "AZURE_CLIENT_ID", "AZURE_CLIENT_SECRET", "AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_CERT_PROFILE"];
    let azure_missing: Vec<&'static str> = AZURE.iter().copied().filter(|k| !set(k)).collect();
    out.push(if set("WINDOWS_CERTIFICATE") {
        Plan { platform: "Windows", ready: true, how: "Authenticode with WINDOWS_CERTIFICATE (.pfx), timestamped".into(), missing: vec![] }
    } else if azure_missing.is_empty() {
        Plan { platform: "Windows", ready: true, how: "Authenticode through Azure Trusted Signing, timestamped".into(), missing: vec![] }
    } else {
        Plan {
            platform: "Windows",
            ready: false,
            how: "unsigned: SmartScreen warns \"Windows protected your PC\"".into(),
            missing: vec!["WINDOWS_CERTIFICATE (or all AZURE_* for Trusted Signing)"],
        }
    });

    // macOS (packaging/macos/package.sh, import-cert.sh in CI): Developer ID, hardened runtime,
    // notarization.
    let identity = set("APPLE_CERTIFICATE") || env("MACOS_SIGN_IDENTITY").is_some_and(|i| !i.trim().is_empty() && i.trim() != "-");
    let notary: Vec<&'static str> = ["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"].into_iter().filter(|k| !set(k)).collect();
    out.push(match (identity, notary.is_empty()) {
        (true, true) => Plan { platform: "macOS", ready: true, how: "Developer ID, hardened runtime, notarized and stapled".into(), missing: vec![] },
        (true, false) => Plan {
            platform: "macOS",
            ready: false,
            how: "Developer ID signed, not notarized: Gatekeeper still asks on first open".into(),
            missing: notary,
        },
        (false, _) => Plan {
            platform: "macOS",
            ready: false,
            how: "ad-hoc signed: Gatekeeper blocks it until Open Anyway".into(),
            missing: [vec!["APPLE_CERTIFICATE (Developer ID Application .p12) or MACOS_SIGN_IDENTITY"], notary].concat(),
        },
    });

    // Linux (packaging/linux/sign.sh): the release GPG key.
    out.push(if set("LINUX_GPG_KEY") {
        Plan { platform: "Linux", ready: true, how: "GPG: .deb signed inside, detached .asc for every package".into(), missing: vec![] }
    } else {
        Plan { platform: "Linux", ready: false, how: "unsigned packages (no .asc to check)".into(), missing: vec!["LINUX_GPG_KEY"] }
    });

    // Every platform's updater (cargo xtask sign-release): SHA256SUMS.sig with the release key.
    out.push(if set("CADADDY_RELEASE_KEY") {
        Plan { platform: "Updates", ready: true, how: "SHA256SUMS.sig with the release key (the updater checks it)".into(), missing: vec![] }
    } else {
        Plan {
            platform: "Updates",
            ready: false,
            how: "no SHA256SUMS.sig: updaters that check signatures (signupdate) refuse the release".into(),
            missing: vec!["CADADDY_RELEASE_KEY"],
        }
    });
    out
}

pub fn run(rest: &[&str]) -> Result<(), String> {
    let require = match rest {
        [] => false,
        ["--require"] => true,
        _ => return Err("usage: cargo xtask signing [--require]".into()),
    };
    let env = |k: &str| std::env::var(k).ok();
    let plans = plans(&env);
    for p in &plans {
        let mark = if p.ready { "ready" } else { "NOT READY" };
        println!("{:<8} {mark:<10} {}", p.platform, p.how);
        if !p.missing.is_empty() {
            println!("{:<19} needs: {}", "", p.missing.join(", "));
        }
    }
    let gaps: Vec<&str> = plans.iter().filter(|p| !p.ready).map(|p| p.platform).collect();
    if require && !gaps.is_empty() {
        return Err(format!("signing: not ready for {} (see docs/code-signing.md)", gaps.join(", ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &[(&str, &str)]) -> Vec<Plan> {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        plans(&move |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()))
    }

    fn ready(plans: &[Plan], platform: &str) -> bool {
        plans.iter().find(|p| p.platform == platform).is_some_and(|p| p.ready)
    }

    #[test]
    fn nothing_set_is_unsigned_everywhere() {
        let p = with(&[]);
        assert!(p.iter().all(|p| !p.ready && !p.missing.is_empty()), "{p:#?}");
        assert!(p.iter().any(|p| p.how.contains("SmartScreen")) && p.iter().any(|p| p.how.contains("Gatekeeper")));
    }

    #[test]
    fn each_platform_is_ready_with_its_material() {
        assert!(ready(&with(&[("WINDOWS_CERTIFICATE", "MIIx")]), "Windows"));
        let azure = [
            ("AZURE_TENANT_ID", "t"),
            ("AZURE_CLIENT_ID", "c"),
            ("AZURE_CLIENT_SECRET", "s"),
            ("AZURE_SIGNING_ENDPOINT", "e"),
            ("AZURE_SIGNING_ACCOUNT", "a"),
            ("AZURE_CERT_PROFILE", "p"),
        ];
        assert!(ready(&with(&azure), "Windows"));
        assert!(!ready(&with(&azure[..5]), "Windows"), "Azure needs all six");
        let notary = [("APPLE_ID", "x"), ("APPLE_PASSWORD", "y"), ("APPLE_TEAM_ID", "z")];
        assert!(ready(&with(&[[("APPLE_CERTIFICATE", "p12")].as_slice(), &notary].concat()), "macOS"));
        assert!(ready(&with(&[[("MACOS_SIGN_IDENTITY", "ABC123")].as_slice(), &notary].concat()), "macOS"));
        assert!(!ready(&with(&[[("MACOS_SIGN_IDENTITY", "-")].as_slice(), &notary].concat()), "macOS"), "ad-hoc isn't signed");
        let signed_only = with(&[("APPLE_CERTIFICATE", "p12")]);
        let mac = signed_only.iter().find(|p| p.platform == "macOS").unwrap();
        assert!(!mac.ready && mac.how.contains("not notarized") && mac.missing == ["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"]);
        assert!(ready(&with(&[("LINUX_GPG_KEY", "LS0t")]), "Linux"));
        assert!(ready(&with(&[("CADADDY_RELEASE_KEY", "/k")]), "Updates"));
        assert!(!ready(&with(&[("LINUX_GPG_KEY", "  ")]), "Linux"), "blank isn't set");
    }

    #[test]
    fn require_fails_while_anything_is_unsigned() {
        assert!(run(&["--bogus"]).is_err());
        // The real environment here has no certificates: plain reporting succeeds.
        if std::env::var_os("WINDOWS_CERTIFICATE").is_none() {
            assert!(run(&[]).is_ok());
            assert!(run(&["--require"]).is_err());
        }
    }
}
