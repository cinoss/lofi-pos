#!/usr/bin/env bash
# Build the bouncer-mock sidecar binary and copy it to the location Tauri
# expects (binaries/bouncer-mock-<target-triple>{.exe?}).
#
# Tauri's `externalBin` declaration in tauri.conf.json points at
# `binaries/bouncer-mock` (no triple, no extension). At bundle/dev time
# Tauri appends the host target triple and resolves the file from disk;
# if it's missing or named without the suffix the build fails.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_TAURI_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKSPACE_ROOT="$(cd "${SRC_TAURI_DIR}/../../.." && pwd)"

TRIPLE="$(rustc -vV | sed -n 's|host: ||p')"
if [[ -z "${TRIPLE}" ]]; then
  echo "build-sidecars.sh: failed to detect host target triple via rustc" >&2
  exit 1
fi

EXT=""
case "${TRIPLE}" in
  *windows*) EXT=".exe" ;;
esac

BIN_NAME="bouncer-mock${EXT}"
OUT_NAME="bouncer-mock-${TRIPLE}${EXT}"
OUT_DIR="${SRC_TAURI_DIR}/binaries"

echo "build-sidecars.sh: building bouncer-mock for ${TRIPLE}"
(cd "${WORKSPACE_ROOT}" && cargo build --release -p bouncer-mock)

mkdir -p "${OUT_DIR}"
SRC_PATH="${WORKSPACE_ROOT}/target/release/${BIN_NAME}"
DST_PATH="${OUT_DIR}/${OUT_NAME}"
cp "${SRC_PATH}" "${DST_PATH}"
chmod +x "${DST_PATH}"
echo "build-sidecars.sh: copied -> ${DST_PATH}"
