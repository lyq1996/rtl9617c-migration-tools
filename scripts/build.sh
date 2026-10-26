#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TARGET=${TARGET:-aarch64-unknown-linux-musl}

command -v cargo >/dev/null 2>&1 || {
	echo "error: cargo not found; put the Rust toolchain in PATH" >&2
	exit 1
}

"$here/scripts/build-module.sh"

echo "==> building $TARGET"
cargo build --release --manifest-path "$here/Cargo.toml" --target "$TARGET"
echo "==> $here/target/$TARGET/release/rtl9617c-migration-tools"
