#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Copyright (C) 2026 M Mustakim Ali
#
# Tests the installer's release-signature check without a network: a fake release is served
# from a directory (file:// URL), and a fake `gh` stands in for the GitHub CLI. Run from the
# repository root: sh tools/test-signature.sh
set -u
cd "$(dirname "$0")/.." || exit 1
root=$(pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
PASS=0
FAIL=0

check() {
  desc=$1; shift
  if "$@"; then
    PASS=$((PASS + 1)); echo "ok   - $desc"
  else
    FAIL=$((FAIL + 1)); echo "FAIL - $desc"
    [ -f "$work/out" ] && sed 's/^/         /' "$work/out"
  fi
}

# A release whose tarball holds a stand-in binary and the real packaging files.
mkdir -p "$work/release" "$work/pkg"
printf '#!/bin/sh\nexit 0\n' > "$work/pkg/heptix"
chmod +x "$work/pkg/heptix"
cp -r "$root/packaging" "$work/pkg/packaging"
case "$(uname -m)" in aarch64 | arm64) arch=aarch64 ;; *) arch=x86_64 ;; esac
name="heptix-$arch-unknown-linux-musl.tar.gz"
tar -czf "$work/release/$name" -C "$work/pkg" heptix packaging
(cd "$work/release" && sha256sum "$name" > "$name.sha256")

# fake_gh <auth-exit> <verify-exit> <verify-output>: a gh on PATH that answers like that.
fake_gh() {
  mkdir -p "$work/bin"
  cat > "$work/bin/gh" <<GH
#!/bin/sh
case "\$1 \$2" in
  "auth status") exit $1 ;;
  "attestation verify") echo "\$@" >> "$work/gh-args"; echo "$3"; exit $2 ;;
esac
exit 99
GH
  chmod +x "$work/bin/gh"
}

# run_install [extra args]: a dry-run direct install from the fake release.
run_install() {
  PATH="$work/bin:$PATH" HEPTIX_DRY_RUN=1 HEPTIX_BASE_URL="file://$work/release" \
    sh "$root/install.sh" --direct --yes "$@" >"$work/out" 2>&1
}

# No gh at all: use a PATH with only the tools the installer needs, then check by name.
mkdir -p "$work/nogh"
for t in sh sed grep tr cat id uname mktemp tar gzip sha256sum curl rm cp chmod mkdir dirname date head systemctl; do
  p=$(command -v "$t" 2>/dev/null) && ln -sf "$p" "$work/nogh/$t"
done

rm -rf "${work:?}/bin"
PATH="$work/nogh" HEPTIX_DRY_RUN=1 HEPTIX_BASE_URL="file://$work/release" \
  sh "$root/install.sh" --direct --yes >"$work/out" 2>&1
check "without gh the install goes on and says the signature was not checked" \
  sh -c "grep -q 'Signature not checked: install the GitHub CLI' '$work/out' && grep -q 'Installing\|+ ' '$work/out'"

fake_gh 1 0 ""
run_install
check "with gh logged out the install goes on and says to log in" \
  sh -c "grep -q 'gh auth login' '$work/out' && ! grep -q 'failed signature' '$work/out'"

fake_gh 0 0 "Loaded digest sha256:abc for file"
rm -f "$work/gh-args"
run_install
check "a verified signature is reported and the install goes on" \
  sh -c "grep -q 'Signature verified' '$work/out'"
check "gh is told to accept only the release workflow as the signer" \
  sh -c "grep -q -- '--signer-workflow mustakimali/heptix/.github/workflows/release.yml' '$work/gh-args' && grep -q -- '--repo mustakimali/heptix' '$work/gh-args'"

fake_gh 0 1 "Error: HTTP 404: Not Found (https://api.github.com/repos/x/y/attestations/sha256:abc)"
HEPTIX_RELEASE_VERSION=0.9.2 run_install
check "a release from before signing started goes on, with a note" \
  sh -c "grep -q 'release 0.9.2 has no attestation' '$work/out' && ! grep -q 'should carry' '$work/out'"

HEPTIX_RELEASE_VERSION=0.9.3 run_install; code=$?
check "the first signed release must have an attestation: a missing one stops the install" \
  sh -c "test $code -ne 0 && grep -q 'should carry a signature' '$work/out' && ! grep -q '^+ ' '$work/out'"

HEPTIX_RELEASE_VERSION=0.10.0 run_install; code=$?
check "a later release must have one too (versions compare as numbers)" \
  sh -c "test $code -ne 0 && grep -q 'should carry a signature' '$work/out'"

HEPTIX_RELEASE_VERSION=0.9.10 run_install; code=$?
check "0.9.10 is newer than 0.9.3, not older" \
  sh -c "test $code -ne 0 && grep -q 'should carry a signature' '$work/out'"

run_install
check "a download from a location you chose goes on, with a note" \
  sh -c "grep -q 'location you chose' '$work/out'"

HEPTIX_RELEASE_VERSION=0.9.3 run_install --no-verify
check "--no-verify still skips the check on a signed release" \
  sh -c "grep -q 'Skipping the signature check' '$work/out' && grep -q '^+ ' '$work/out'"

fake_gh 0 1 "X Verification failed: the digest does not match"
run_install; code=$?
check "a signature that does not verify stops the install" \
  sh -c "test $code -ne 0 && grep -q 'failed signature verification' '$work/out' && ! grep -q '^+ ' '$work/out'"

run_install --no-verify
check "--no-verify skips the check even when the signature is bad" \
  sh -c "grep -q 'Skipping the signature check' '$work/out' && ! grep -q 'failed signature' '$work/out'"

HEPTIX_NO_VERIFY=1 run_install
check "HEPTIX_NO_VERIFY=1 does the same" \
  sh -c "grep -q 'Skipping the signature check' '$work/out'"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
