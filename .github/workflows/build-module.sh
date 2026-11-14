#!/bin/sh
set -eu

: "${SERIES:?}"
: "${VARIANT:?}"
: "${VERSION:?}"
: "${URL:?}"
: "${SHA256:?}"
: "${CONFIG:?}"
: "${EXPECTED_MODULE_SIZE:?}"
: "${GITHUB_WORKSPACE:?}"

ARCH=${ARCH:-arm64}
CROSS_COMPILE=${CROSS_COMPILE:-aarch64-linux-gnu-}
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/rtl9617c-kernels
WORK=${RUNNER_TEMP:-/tmp}/rtl9617c-$SERIES-$VARIANT

mkdir -p "$CACHE"
TARBALL="$CACHE/linux-$VERSION.tar.xz"
if [ ! -f "$TARBALL" ]; then
	curl -fL --retry 3 --retry-delay 2 -o "$TARBALL" "$URL"
fi
echo "$SHA256  $TARBALL" | sha256sum -c -

rm -rf "$WORK"
mkdir -p "$WORK"
tar -xf "$TARBALL" -C "$WORK"
K="$WORK/linux-$VERSION"
cd "$K"

make ARCH="$ARCH" CROSS_COMPILE="$CROSS_COMPILE" defconfig
scripts/kconfig/merge_config.sh -m .config "$GITHUB_WORKSPACE/$CONFIG"
make ARCH="$ARCH" CROSS_COMPILE="$CROSS_COMPILE" olddefconfig

if grep -q '^CONFIG_MODVERSIONS=y' .config; then
	echo "error: CONFIG_MODVERSIONS must be disabled" >&2
	exit 1
fi
if ! grep -q '^CONFIG_LOCALVERSION="-rtl9617c-module-slot-' .config; then
	echo "error: CONFIG_LOCALVERSION slot is missing" >&2
	exit 1
fi

make -j"$(nproc)" ARCH="$ARCH" CROSS_COMPILE="$CROSS_COMPILE" modules_prepare
make -j"$(nproc)" ARCH="$ARCH" CROSS_COMPILE="$CROSS_COMPILE" KBUILD_MODPOST_WARN=1 \
	M="$GITHUB_WORKSPACE/modules/src" modules

BUILT="$GITHUB_WORKSPACE/modules/src/rtl9617c_mtd_rw.ko"
[ -f "$BUILT" ] || {
	echo "error: $BUILT was not produced" >&2
	exit 1
}

python3 - "$BUILT" "$EXPECTED_MODULE_SIZE" "$SERIES" <<'PY'
import re
import subprocess
import sys

path, expected, series = sys.argv[1], sys.argv[2], sys.argv[3]


def readelf(*args):
    return subprocess.run(
        ["readelf", *args, path], capture_output=True, text=True, check=True
    ).stdout


header = readelf("-hW")
machine = re.search(r"Machine:\s+(.+)", header).group(1).strip()
if "AArch64" not in machine:
    sys.exit(f"error: module is {machine}, not AArch64")

section_line = re.compile(
    r"^\s*\[\s*\d+\]\s+(\S+)\s+(\S+)\s+[0-9a-fA-F]+\s+[0-9a-fA-F]+\s+"
    r"([0-9a-fA-F]+)\s+\S+\s+(\S*)\s+\d+\s+\d+\s+(\d+)"
)
sections = [m.groups() for m in map(section_line.match, readelf("-SW").splitlines()) if m]
this_module = [s for s in sections if s[0] == ".gnu.linkonce.this_module"]

if len(this_module) != 1:
    sys.exit("error: module must have exactly one this_module section")

size = int(this_module[0][2], 16)
if str(size) != expected:
    sys.exit(f"error: this_module is {size}, expected {expected}")

info = readelf("-p", ".modinfo")
name = re.search(r"(?m)^\s*\[\s*[0-9a-f]+\]\s+name=(\S+)", info)
vermagic = re.search(r"(?m)^\s*\[\s*[0-9a-f]+\]\s+vermagic=(\S+)", info)
if not name or name.group(1) != "rtl9617c_mtd_rw":
    sys.exit("error: .modinfo is missing the module name")
if not vermagic:
    sys.exit("error: .modinfo is missing vermagic")
if ".".join(vermagic.group(1).split(".")[:2]) != series:
    sys.exit(f"error: vermagic {vermagic.group(1)} is not series {series}")

undefined = {
    line.split()[-1]
    for line in subprocess.run(
        ["nm", "-u", path], capture_output=True, text=True, check=True
    ).stdout.splitlines()
    if line.split()
}
unexpected = sorted(undefined - {"_printk", "printk", "param_ops_ulong"})
if unexpected:
    sys.exit(f"error: unexpected undefined symbols: {' '.join(unexpected)}")

print(f"    machine          : {machine}")
print(f"    this_module size : {size}")
print(f"    vermagic         : {vermagic.group(1)}")
PY

"${CROSS_COMPILE}strip" --strip-debug "$BUILT"
mkdir -p "$GITHUB_WORKSPACE/modules"
cp "$BUILT" "$GITHUB_WORKSPACE/modules/$SERIES-$VARIANT.ko"
rm -rf "$WORK"
echo "==> $GITHUB_WORKSPACE/modules/$SERIES-$VARIANT.ko"
