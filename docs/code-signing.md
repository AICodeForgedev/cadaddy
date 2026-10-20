# Code signing

Everything is ready to sign; only the certificates are missing. Until they exist the packaging scripts
build unsigned (or ad-hoc signed) packages and say so. Once a certificate is in the environment, that
platform is signed with nothing else to change.

**Before a release:** `cargo xtask signing` lists what each platform would get. Once the certificates
exist, the release script runs `cargo xtask signing --require`, which stops the release if anything would
ship unsigned.

## Windows: Authenticode (`packaging/windows/sign.ps1`, called by `package.ps1`)

One of these:

| Variable | What |
|---|---|
| `WINDOWS_CERTIFICATE` | the code-signing certificate, a `.pfx` file base64-encoded |
| `WINDOWS_CERTIFICATE_PASSWORD` | its password |
| *or* `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE` | Azure Trusted Signing |

The exe and the MSI are signed with SHA-256, timestamped (`WINDOWS_TIMESTAMP_URL` overrides the server),
and verified with `signtool verify /pa`.

**Unsigned:** SmartScreen says "Windows protected your PC" and the user has to click More info ▸ Run anyway.
A new certificate also needs some download reputation before the warning goes away (an EV certificate or
Trusted Signing earns it faster).

## macOS: Developer ID and notarization (`packaging/macos/package.sh`, `import-cert.sh`)

| Variable | What |
|---|---|
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` | the Developer ID Application certificate (`.p12`, base64): `import-cert.sh` puts it in a temporary keychain and sets `MACOS_SIGN_IDENTITY` |
| or `MACOS_SIGN_IDENTITY` (`MACOS_KEYCHAIN`) | an identity already in a keychain |
| `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password), `APPLE_TEAM_ID` | notarization with `notarytool` |

The app and CLI are signed with the hardened runtime (`entitlements.plist`) and a secure timestamp. The app,
the DMG and the CLI zip are notarized; the app and DMG are stapled, then checked with `spctl`.

**Unsigned (ad-hoc):** Gatekeeper says the app "can't be opened" or "Apple could not verify…", and the
user has to go to System Settings ▸ Privacy & Security ▸ Open Anyway. Signed but not notarized, it still asks.

## Linux: GPG (`packaging/linux/sign.sh`, called by `package.sh`)

| Variable | What |
|---|---|
| `LINUX_GPG_KEY` | the release key: an armoured secret key, base64-encoded |
| `LINUX_GPG_PASSPHRASE` | its passphrase |

The `.deb` is signed inside (`debsigs` "origin", or `dpkg-sig` where only that is installed). Every package
(`.deb`, `.rpm`, `.tar.gz`, AppImage) gets a detached `FILE.asc`, verified as it's made. After
`cargo xtask sign-release`, run `packaging/linux/sign.sh dist/release/SHA256SUMS` too, so Linux users can
check the sums with GPG. Publish the public key with the release (`gpg --armor --export <key>`), and on
the website.

The key only ever exists in a temporary GNUPGHOME that is deleted afterwards.

**Unsigned:** nothing stops installation, but there is nothing to check the download against.

## Updates: `SHA256SUMS.sig` (`cargo xtask sign-release`, see docs/release-signing.md once signupdate merges)

The release key (`CADADDY_RELEASE_KEY`) signs SHA256SUMS for the built-in updater, separately from the
platforms' code signing.

## Getting the certificates

| What | From | Cost (approximate; check current prices) |
|---|---|---|
| Apple Developer Program (Developer ID + notarization) | developer.apple.com | US$99 a year |
| Windows code signing | Azure Trusted Signing, or an OV/EV certificate from a CA | Trusted Signing about US$10 a month; OV certificates roughly US$200–400 a year, EV more (hardware token or cloud HSM) |
| Linux GPG key | made locally (`gpg --quick-gen-key`) | free |

Keep every key out of the repository: set them only in the release environment's secrets.
