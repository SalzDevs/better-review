#!/usr/bin/env sh
set -eu

REPO="${BETTER_REVIEW_REPO:-Ricardo-Ceia/better-review}"
VERSION="${BETTER_REVIEW_VERSION:-latest}"
PREFIX="${BETTER_REVIEW_INSTALL_PREFIX:-}"
BIN_DIR="${BETTER_REVIEW_BIN_DIR:-}"
BINARY="better-review"

say() {
  printf "%s\n" "$*"
}

fail() {
  printf "error: %s\n" "$*" >&2
  exit 1
}

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "${os}" in
    Linux)
      case "${arch}" in
        x86_64) printf "x86_64-unknown-linux-gnu" ;;
        *) fail "unsupported Linux architecture: ${arch} (supported: x86_64)" ;;
      esac
      ;;
    Darwin)
      case "${arch}" in
        x86_64) printf "x86_64-apple-darwin" ;;
        arm64|aarch64) printf "aarch64-apple-darwin" ;;
        *) fail "unsupported macOS architecture: ${arch} (supported: x86_64, arm64)" ;;
      esac
      ;;
    *)
      fail "unsupported operating system: ${os}"
      ;;
  esac
}

install_dir() {
  if [ -n "${BIN_DIR}" ]; then
    printf "%s" "${BIN_DIR}"
    return
  fi

  if [ -n "${PREFIX}" ]; then
    printf "%s/bin" "${PREFIX}"
    return
  fi

  if [ -w "/usr/local/bin" ]; then
    printf "/usr/local/bin"
    return
  fi

  printf "%s/.local/bin" "${HOME}"
}

normalize_version() {
  if [ "${VERSION}" = "latest" ]; then
    printf "latest"
    return
  fi

  case "${VERSION}" in
    v*) printf "%s" "${VERSION}" ;;
    *) printf "v%s" "${VERSION}" ;;
  esac
}

TARGET="$(detect_target)"
SELECTED_VERSION="$(normalize_version)"
ARCHIVE="${BINARY}-${TARGET}.tar.gz"

if [ "${SELECTED_VERSION}" = "latest" ]; then
  BASE_URL="https://github.com/${REPO}/releases/latest/download"
else
  BASE_URL="https://github.com/${REPO}/releases/download/${SELECTED_VERSION}"
fi

ARCHIVE_URL="${BASE_URL}/${ARCHIVE}"
CHECKSUM_URL="${ARCHIVE_URL}.sha256"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "${TMPDIR}"' EXIT INT TERM

ARCHIVE_PATH="${TMPDIR}/${ARCHIVE}"
CHECKSUM_PATH="${ARCHIVE_PATH}.sha256"

say "Downloading ${ARCHIVE_URL}"
curl -fsSL "${ARCHIVE_URL}" -o "${ARCHIVE_PATH}" || fail "download failed"

if curl -fsSL "${CHECKSUM_URL}" -o "${CHECKSUM_PATH}"; then
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "${TMPDIR}" && sha256sum -c "${ARCHIVE}.sha256") || fail "checksum verification failed"
  elif command -v shasum >/dev/null 2>&1; then
    expected="$(awk '{print $1}' "${CHECKSUM_PATH}")"
    actual="$(shasum -a 256 "${ARCHIVE_PATH}" | awk '{print $1}')"
    [ "${expected}" = "${actual}" ] || fail "checksum verification failed"
  else
    say "warning: no checksum tool found; skipping verification"
  fi
else
  say "warning: checksum file not found; skipping verification"
fi

tar -xzf "${ARCHIVE_PATH}" -C "${TMPDIR}" || fail "failed to extract archive"
[ -f "${TMPDIR}/${BINARY}" ] || fail "archive did not contain ${BINARY}"

DEST_DIR="$(install_dir)"
mkdir -p "${DEST_DIR}" || fail "failed to create ${DEST_DIR}"
DEST_PATH="${DEST_DIR}/${BINARY}"

cp "${TMPDIR}/${BINARY}" "${DEST_PATH}" || fail "failed to install binary to ${DEST_PATH}"
chmod +x "${DEST_PATH}" || fail "failed to set executable bit"

say "Installed ${BINARY} to ${DEST_PATH}"
case ":${PATH}:" in
  *":${DEST_DIR}:"*)
    ;;
  *)
    say "Add ${DEST_DIR} to PATH to run ${BINARY} from anywhere."
    ;;
esac
say "Run: ${BINARY}"
