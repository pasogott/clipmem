#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
config_file="${repo_root}/dist-workspace.toml"
install_dir="${1:-${HOME}/.cargo/bin}"

if [[ ! -f "${config_file}" ]]; then
  echo "dist config not found: ${config_file}" >&2
  exit 1
fi

version="$(
  sed -nE 's/^cargo-dist-version = "([^"]+)"$/\1/p' "${config_file}" | head -n 1
)"

if [[ -z "${version}" ]]; then
  echo "failed to read cargo-dist-version from ${config_file}" >&2
  exit 1
fi

os="$(uname -s)"
arch="$(uname -m)"

case "${os}:${arch}" in
  Linux:x86_64)
    asset="cargo-dist-x86_64-unknown-linux-gnu.tar.xz"
    expected_sha256="4b3f0a5f0ebbdb798f6db649d01b32ba1518376b6f7a0502b7d92b75cc2c8293"
    ;;
  Darwin:x86_64)
    asset="cargo-dist-x86_64-apple-darwin.tar.xz"
    expected_sha256="6a49bfb61bd86770d79c27f3d2b40c6b2e71cde940d3d31a6ccaaffc124d7a29"
    ;;
  Darwin:arm64|Darwin:aarch64)
    asset="cargo-dist-aarch64-apple-darwin.tar.xz"
    expected_sha256="7b3cbe25511de01d74c0f5fcb7909edabd379bea9cfa284d93af5a3cdfa3247c"
    ;;
  *)
    echo "unsupported runner platform for cargo-dist bootstrap: ${os}/${arch}" >&2
    echo "add an audited archive + checksum mapping before enabling this runner" >&2
    exit 1
    ;;
esac

case "${version}" in
  0.33.0)
    ;;
  *)
    echo "unsupported cargo-dist version: ${version}" >&2
    echo "add audited archive hashes for ${version} before changing cargo-dist-version" >&2
    exit 1
    ;;
esac

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

archive_path="${tmpdir}/${asset}"
download_url="https://github.com/axodotdev/cargo-dist/releases/download/v${version}/${asset}"

echo "Installing cargo-dist ${version} from ${asset}"
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  --output "${archive_path}" \
  "${download_url}"

if command -v sha256sum >/dev/null 2>&1; then
  actual_sha256="$(sha256sum "${archive_path}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual_sha256="$(shasum -a 256 "${archive_path}" | awk '{print $1}')"
else
  echo "missing sha256 tool; expected sha256sum or shasum" >&2
  exit 1
fi

if [[ "${actual_sha256}" != "${expected_sha256}" ]]; then
  echo "checksum mismatch for ${asset}" >&2
  echo "expected: ${expected_sha256}" >&2
  echo "actual:   ${actual_sha256}" >&2
  exit 1
fi

mkdir -p "${install_dir}"
tar -xf "${archive_path}" -C "${tmpdir}"

dist_path="$(find "${tmpdir}" -type f -name dist | head -n 1)"
if [[ -z "${dist_path}" ]]; then
  echo "failed to locate dist binary in ${asset}" >&2
  exit 1
fi

install -m 0755 "${dist_path}" "${install_dir}/dist"
export PATH="${install_dir}:${PATH}"

if [[ -n "${GITHUB_PATH:-}" ]]; then
  echo "${install_dir}" >> "${GITHUB_PATH}"
fi

"${install_dir}/dist" --version
