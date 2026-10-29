#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
: "${KDIR:?set KDIR to a prepared kernel build tree}"
CROSS_COMPILE=${CROSS_COMPILE:-aarch64-openwrt-linux-musl-}
ARCH=${ARCH:-arm64}
MODULE_TREE_LOOKUP=${MODULE_TREE_LOOKUP:-1}
TARGET_RELEASE=${TARGET_RELEASE:-$(cat "$KDIR/include/config/kernel.release")}
# One module per kernel series: 6.18.55 -> modules/6.18.ko. The loader
# re-stamps the vermagic release to the running kernel at load time.
SERIES=$(printf '%s' "$TARGET_RELEASE" | cut -d. -f1,2)
OUT=${OUT:-$here/modules/$SERIES.ko}

EXPECTED_MODULE_SIZE=${EXPECTED_MODULE_SIZE:-}

command -v "${CROSS_COMPILE}gcc" >/dev/null 2>&1 || {
	echo "error: ${CROSS_COMPILE}gcc not found; put the target toolchain in PATH" >&2
	exit 1
}

kcflags=
if [ "$MODULE_TREE_LOOKUP" = 1 ]; then
	kcflags="-DCONFIG_MODULES_TREE_LOOKUP=1"
fi

echo "==> building rtl9617c_mtd_rw.ko"
echo "    kernel tree : $KDIR ($(cat "$KDIR/include/config/kernel.release"))"
echo "    target      : $TARGET_RELEASE"
make -C "$KDIR" M="$here/modules/src" ARCH="$ARCH" CROSS_COMPILE="$CROSS_COMPILE" \
	KCFLAGS="$kcflags" modules

built=$here/modules/src/rtl9617c_mtd_rw.ko
[ -f "$built" ] || { echo "error: $built was not produced" >&2; exit 1; }

echo "==> adapting vermagic to $TARGET_RELEASE"
python3 - "$built" "$TARGET_RELEASE" <<'PY'
import re
import sys

path, target = sys.argv[1], sys.argv[2]
data = open(path, "rb").read()
match = re.search(rb"vermagic=([^\x00]+)\x00", data)
if not match:
    sys.exit("error: the module carries no vermagic")
current = match.group(1).decode()
release = current.split(" ")[0]
if release == target:
    print(f"    vermagic already {current}")
    raise SystemExit(0)
if len(release) != len(target):
    sys.exit(
        f"error: cannot rewrite vermagic {current!r} to {target!r}: the "
        "release strings differ in length. Rebuild inside a kernel tree whose "
        "release is already the target one."
    )
patched = current.replace(release, target, 1).encode()
data = data[: match.start(1)] + patched + data[match.end(1) :]
open(path, "wb").write(data)
print(f"    {current} -> {patched.decode()}")
PY

echo "==> stripping debug information"
"${CROSS_COMPILE}strip" --strip-debug "$built"

echo "==> checking what the loader checks"
python3 - "$built" "$EXPECTED_MODULE_SIZE" <<'PY'
import re
import subprocess
import sys

path, expected = sys.argv[1], sys.argv[2]


def readelf(*args):
    return subprocess.run(
        ["readelf", *args, path], capture_output=True, text=True, check=True
    ).stdout


header = readelf("-hW")
machine = re.search(r"Machine:\s+(.+)", header).group(1).strip()
if "AArch64" not in machine:
    sys.exit(f"error: the module is built for {machine}, not AArch64")

section_line = re.compile(
    r"^\s*\[\s*\d+\]\s+(\S+)\s+(\S+)\s+[0-9a-fA-F]+\s+[0-9a-fA-F]+\s+"
    r"([0-9a-fA-F]+)\s+\S+\s+(\S*)\s+\d+\s+\d+\s+(\d+)"
)
sections = [m.groups() for m in map(section_line.match, readelf("-SW").splitlines()) if m]
this_module = [s for s in sections if s[0] == ".gnu.linkonce.this_module"]
modinfo = [s for s in sections if s[0] == ".modinfo"]

if len(this_module) != 1:
    sys.exit("error: the module must have exactly one this_module section")
if len(modinfo) != 1:
    sys.exit("error: the module must have exactly one .modinfo section")

name, kind, size, flags, align = this_module[0]
size, align = int(size, 16), int(align)
if kind == "NOBITS":
    sys.exit("error: the this_module section has no contents")
if "A" not in flags:
    sys.exit("error: the this_module section is not allocated")
if align != 64:
    sys.exit(f"error: the this_module section is aligned to {align}, expected 64")

info = readelf("-p", ".modinfo")
name = re.search(r"(?m)^\s*\[\s*[0-9a-f]+\]\s+name=(\S+)", info)
vermagic = re.search(r"(?m)^\s*\[\s*[0-9a-f]+\]\s+vermagic=(.+?)\s*$", info)
if not name or not vermagic:
    sys.exit("error: .modinfo is missing name or vermagic")

relocs = readelf("-rW")
section = None
offsets = {}
for line in relocs.splitlines():
    match = re.match(r"Relocation section '([^']+)'", line)
    if match:
        section = match.group(1)
        continue
    if section != ".rela.gnu.linkonce.this_module":
        continue
    entry = line.split()
    if len(entry) >= 5 and entry[2].startswith("R_AARCH64"):
        offsets[entry[4]] = int(entry[0], 16)

print(f"    machine          : {machine}")
print(f"    this_module size : {size}")
print(f"    this_module flags: {flags}, align {align}")
for symbol in ("init_module", "cleanup_module"):
    value = offsets.get(symbol)
    print(f"    {symbol:16s} : {'absent' if value is None else hex(value)}")
print(f"    vermagic         : {vermagic.group(1)}")

if expected and str(size) != expected:
    sys.exit(
        f"error: sizeof(struct module) is {size}, the target kernel needs "
        f"{expected}; check MODULE_TREE_LOOKUP for the target kernel"
    )
PY

mkdir -p "$(dirname "$OUT")"
if [ "$OUT" != "$built" ]; then
	cp "$built" "$OUT"
fi
echo "==> $OUT"
