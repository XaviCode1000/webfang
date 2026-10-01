#!/usr/bin/env bash
# verify-release-signatures.sh — fail-closed keyless-signature verification for
# every asset this repository publishes (issue #1607, SC-15, SC-17).
#
# WHAT THIS IS FOR
# ================
# A `SHA256SUMS.txt` written by the same job that built the binaries is not
# evidence of anything: whoever could tamper with the archives could rewrite the
# list. The release pipeline now signs every published asset with cosign's
# KEYLESS flow (sigstore + the GitHub Actions OIDC issuer), so the checksum
# manifest is signed by an identity that is independent of the job that produced
# it. This script is the consumer side of that property, and the release's own
# gate: `publish` runs it BEFORE `gh release create`, so a release whose assets
# are unsigned, tampered, or signed by a foreign identity is never created.
#
# A MISSING OR INVALID BUNDLE BLOCKS THE RELEASE. That is the entire point, and
# it is why there is no "cannot verify, carry on" branch anywhere below:
#   * cosign not on PATH           -> FAIL (never a skip). An absent verifier
#                                     that reads as "nothing to check" is how an
#                                     unsigned release ships with a green job.
#   * bundle file missing          -> FAIL. An asset with no bundle is unsigned.
#   * bundle present, signature
#     invalid or identity foreign  -> FAIL. cosign's own non-zero exit.
#   * no assets passed at all      -> FAIL. An empty verification is not a pass.
#
# THE TRUST BOUNDARY, STATED HONESTLY
# ====================================
# Keyless means there is no long-lived signing key, so the signing identity is
# the WORKFLOW PATH, not a person: the certificate's SAN is
#     https://github.com/<owner>/<repo>/.github/workflows/release.yml@refs/tags/vX.Y.Z
# and the trust anchor is the public Rekor transparency log, with the
# certificate minted by the GitHub Actions OIDC issuer. Consequently:
#   * A consumer trusting `--certificate-identity-regexp` is trusting "this
#     workflow file, at a version tag, in this repository, signed during the
#     run that produced that tag".
#   * A consumer is NOT getting a person or an org signature, and this script
#     must never be described as if they were. The regex below is what pins the
#     boundary, and it is printed on every run so the value under test is
#     visible rather than implied.
#   * A long-lived key (cosign key-pair, KMS) would be strictly stronger for
#     consumers who want a stable identity, and needs a maintainer-held secret.
#     That secret cannot be provisioned from inside this repository, which is
#     why the keyless design is the one that can actually be landed.
#
# USAGE
#   bash scripts/verify-release-signatures.sh --help
#   bash scripts/verify-release-signatures.sh [--identity-regexp RE] <asset> [asset ...]
#
# Each <asset> must have a sibling bundle named <asset>.sigstore.json, as
# produced by `cosign sign-blob --bundle <asset>.sigstore.json <asset>`.
#
# ENVIRONMENT OVERRIDES
#   RELEASE_CERT_IDENTITY_REGEXP   identity regexp (same effect as the flag)
#   COSIGN                         path to the cosign binary (default: cosign on PATH)
#
# STYLE: user-facing errors are Spanish (repo convention for this scripts/
# family); internal output is English.

set -euo pipefail

export LC_ALL=C

# The pinned identity. Pinned to the release workflow of this repository at a
# version tag: a bundle minted by any other workflow, any other repository, or
# any non-tag ref is a DIFFERENT identity and is rejected. Overridable so a fork
# can verify its own releases without editing this file, but the value in force
# is printed on every run.
DEFAULT_IDENTITY_REGEXP='^https://github\.com/XaviCode1000/webfang/\.github/workflows/release\.yml@refs/tags/v[0-9].*$'

# The OIDC issuer. Pinned, not derived: a certificate from any other issuer is
# not an Actions keyless certificate.
OIDC_ISSUER='https://token.actions.githubusercontent.com'

usage() {
    cat <<'USAGE'
verify-release-signatures.sh — verify every published release asset against its
keyless (sigstore + GitHub OIDC) cosign signature bundle. Fail-closed: a missing
cosign binary, a missing bundle, an invalid signature or a foreign identity all
block the release.

usage:
  bash scripts/verify-release-signatures.sh --help
  bash scripts/verify-release-signatures.sh [--identity-regexp RE] <asset> [asset ...]

Each <asset> requires a sibling bundle named <asset>.sigstore.json, produced by
  cosign sign-blob --bundle <asset>.sigstore.json <asset>

options:
  --identity-regexp RE   Override the pinned certificate identity regexp.
                         Default: the release workflow of XaviCode1000/webfang
                         at a version tag (see the constant in this script).
  -h, --help             Print this help and exit 0 (no cosign required).

environment:
  RELEASE_CERT_IDENTITY_REGEXP   same override as --identity-regexp
  COSIGN                         path to the cosign binary (default: cosign)

exit codes:
  0  every asset verified against its bundle
  1  verification failed (missing tool, missing bundle, bad signature, foreign
     identity, or no assets given)
  2  usage error
USAGE
}

IDENTITY_REGEXP="${RELEASE_CERT_IDENTITY_REGEXP:-$DEFAULT_IDENTITY_REGEXP}"
COSIGN_BIN="${COSIGN:-cosign}"
ASSETS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        --identity-regexp)
            if [[ $# -lt 2 ]]; then
                echo "::error::--identity-regexp requiere un valor." >&2
                exit 2
            fi
            IDENTITY_REGEXP="$2"
            shift 2
            ;;
        --identity-regexp=*)
            IDENTITY_REGEXP="${1#--identity-regexp=}"
            shift
            ;;
        --)
            shift
            while [[ $# -gt 0 ]]; do
                ASSETS+=("$1")
                shift
            done
            ;;
        -*)
            echo "::error::opción desconocida: $1" >&2
            usage >&2
            exit 2
            ;;
        *)
            ASSETS+=("$1")
            shift
            ;;
    esac
done

if [[ ${#ASSETS[@]} -eq 0 ]]; then
    echo "::error::no se pasó ningún asset que verificar: una verificación vacía no es una verificación correcta. Lista los assets a comprobar (o usa --help)."
    exit 1
fi

# The value under test, printed. A pin nobody can read is not a pin.
echo "certificate identity regexp: $IDENTITY_REGEXP"
echo "certificate oidc issuer:     $OIDC_ISSUER"

# Fail-closed on a missing verifier. Deliberately BEFORE any per-asset work, so
# the failure cannot be confused with "there was nothing to verify".
if ! command -v "$COSIGN_BIN" >/dev/null 2>&1; then
    echo "::error::cosign no está disponible (probado: '$COSIGN_BIN') y este guard NO se salta: sin verificador, una release sin firma pasaría en verde. En CI lo instala 'sigstore/cosign-installer' (version pineada vía 'cosign-release'); en local, descarga el binario desde https://github.com/sigstore/cosign/releases."
    exit 1
fi

if ! "$COSIGN_BIN" version >/dev/null 2>&1; then
    echo "::error::'$COSIGN_BIN' está en PATH pero no responde a 'version' — no es un cosign utilizable. Bloqueando la release en lugar de omitir la verificación."
    exit 1
fi

VERIFIED=0
FAILED=0

for asset in "${ASSETS[@]}"; do
    if [[ ! -f "$asset" ]]; then
        echo "::error::$asset no existe: no se puede verificar un asset ausente." >&2
        FAILED=$((FAILED + 1))
        continue
    fi

    bundle="${asset}.sigstore.json"
    if [[ ! -f "$bundle" ]]; then
        echo "::error::$asset no tiene bundle '$bundle': el asset está SIN FIRMAR. Un bundle ausente o inválido bloquea la release — no se publica nada sin evidencia de firma." >&2
        FAILED=$((FAILED + 1))
        continue
    fi

    if "$COSIGN_BIN" verify-blob \
        --bundle "$bundle" \
        --certificate-identity-regexp "$IDENTITY_REGEXP" \
        --certificate-oidc-issuer "$OIDC_ISSUER" \
        "$asset"; then
        echo "OK signature verified: $asset"
        VERIFIED=$((VERIFIED + 1))
    else
        echo "::error::firma inválida, ausente en el log de transparencia, o de una identidad ajena: $asset. No se publica." >&2
        FAILED=$((FAILED + 1))
    fi
done

TOTAL=${#ASSETS[@]}
echo "release signature verification: $VERIFIED/$TOTAL verified, $FAILED failed"

if [[ "$FAILED" -ne 0 ]]; then
    echo "::error::$FAILED de $TOTAL assets no superaron la verificación de firma. La release NO se crea: publicar un asset sin firma verificable sería exactamente el defecto que este guard existe para cerrar."
    exit 1
fi

if [[ "$VERIFIED" -ne "$TOTAL" ]]; then
    # Unreachable given the accounting above, and kept anyway: a counter that
    # does not reconcile must not read as a pass.
    echo "::error::la contabilidad no cuadra ($VERIFIED verificados de $TOTAL assets) — fallando en vez de asumir éxito."
    exit 1
fi
