#!/usr/bin/env bash
# Sign Linux release files with the release GPG key, so users can check them with `gpg --verify`.
#
#   packaging/linux/sign.sh FILE...
#
# Each FILE gets a detached, armoured signature FILE.asc; a .deb is also signed inside (debsigs's
# "origin" signature, or dpkg-sig's "builder" one where only that is installed). Run it over the
# packages and, after `cargo xtask sign-release` (the updater's own signature), over SHA256SUMS.
#
# Key (env):
#   LINUX_GPG_KEY          the release signing key: an armoured secret key, base64-encoded
#   LINUX_GPG_PASSPHRASE   its passphrase (optional)
# Without LINUX_GPG_KEY it says so and leaves the files unsigned (exit 0), so test builds still work.
# The key lives only in a temporary GNUPGHOME, deleted afterwards. Every signature is verified.
set -euo pipefail
umask 077
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

[ "$#" -gt 0 ] || { echo "usage: $0 FILE..." >&2; exit 2; }

if [ -z "${LINUX_GPG_KEY:-}" ]; then
  warn "LINUX_GPG_KEY is not set: leaving unsigned: $*"
  exit 0
fi
command -v gpg >/dev/null || { echo "error: gpg not found (apt install gnupg)" >&2; exit 1; }

GNUPGHOME="$(mktemp -d "${TMPDIR:-/tmp}/cadaddy-gpg.XXXXXX")"
export GNUPGHOME
trap 'gpgconf --kill gpg-agent 2>/dev/null || true; rm -rf "$GNUPGHOME"' EXIT

printf '%s' "$LINUX_GPG_KEY" | base64 --decode | gpg --batch --quiet --import
KEY="$(gpg --batch --list-secret-keys --with-colons | awk -F: '/^fpr:/ { print $10; exit }')"
[ -n "$KEY" ] || { echo "error: LINUX_GPG_KEY holds no secret key" >&2; exit 1; }
echo "==> signing with $KEY"

# The passphrase, for every gpg this runs (debsigs and dpkg-sig call gpg themselves): in this
# temporary home only, readable by us only, deleted with it.
if [ -n "${LINUX_GPG_PASSPHRASE:-}" ]; then
  printf '%s' "$LINUX_GPG_PASSPHRASE" >"$GNUPGHOME/passphrase"
  printf 'batch\npinentry-mode loopback\npassphrase-file %s\n' "$GNUPGHOME/passphrase" >"$GNUPGHOME/gpg.conf"
fi

for f in "$@"; do
  [ -f "$f" ] || { echo "error: $f: no such file" >&2; exit 1; }
  case "$f" in
    *.deb)
      if command -v debsigs >/dev/null; then
        debsigs --sign=origin --default-key="$KEY" "$f"
        echo "signed inside (debsigs origin): $f"
      elif command -v dpkg-sig >/dev/null; then
        dpkg-sig --sign builder -k "$KEY" "$f"
        dpkg-sig --verify "$f"
        echo "signed inside (dpkg-sig builder): $f"
      else
        warn "neither debsigs nor dpkg-sig is installed: $f gets only a detached signature"
      fi
      ;;
  esac
  gpg --batch --yes --local-user "$KEY" --armor --detach-sign --output "$f.asc" "$f"
  gpg --batch --verify "$f.asc" "$f" 2>/dev/null || { echo "error: $f.asc doesn't verify" >&2; exit 1; }
  echo "wrote $f.asc"
done
