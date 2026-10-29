# RTL9617C Migration Tools


适配机型：
1. PT1500-H10  2.5G版本；
2. TYFZA1  2.5G版本；

## 使用
1. 上传`rtl9617c-migration-tools`到设备的`/tmp`目录

```sh
chmod +x /tmp/rtl9617c-migration-tools
/tmp/rtl9617c-migration-tools
```
2. 打开 `http://<设备IP>:3333/`，下载备份；
3. 上传本机型的 `u-boot.bin`：工具会解析镜像，BL2/BL31 与本机 `mtd0` 一致才允许刷入；
4. 如需清掉原厂环境变量，在刷入页点击 **清除 env 分区**（一次擦除 `env` 与 `env2`）；
5. 使用[rtl9617c-factory](https://github.com/lyq1996/rtl9617c-factory)组装Factory分区；
6. 重启后访问`http://192.168.0.1/` 继续，重建UBI -> 恢复Factory -> 刷入固件。

完整流程：**备份 → 刷写 mtd0 → 重启**，env/env2 仅在主动点击清除按钮时擦除。

### 清除 env 分区
新 U-Boot 从 mtd1 读取标准 U-Boot env，为更好的支持新U-Boot，建议清除原厂 env（mtd1/mtd2）。

部分机型的 MAC、序列号等设备身份信息只保留在备份文件里，需要使用`rtl9617c-factory`迁移。

⚠️：
- 清除按钮一次擦除 `env` 与 `env2` 两个分区；
- 与刷入共用忙锁，任一操作进行中另一个都会被拒绝；
- 没有分区可清（本机不存在 env/env2）时按钮不可用。

## 操作范围
只写 `boot`（mtd0）分区；`env` / `env2` 仅在点击清除按钮时整分区擦除

## 备份
只提供物理分区下载。`ubi_*` 是 `ubi_device` 内部的卷，系统占用时打不开，
读出来也不是可恢复的镜像；整机备份请随后进入uboot进行。

## 机型
```ini
name       = PT1500-H10
compatible = youhua,pt1500
bl2        = <BL2 payload 的 SHA-256>
bl31       = <BL31 payload 的 SHA-256>
```

识别与刷入都基于 `mtd0` 的 FIP payload：读取本机 boot 分区，解析 FIP 后取 BL2
的 SHA-256 查 `boards/*.conf` 得出机型（支持按 BL2 细分同板不同硬件变体）；
BL2 与 BL31 两个 payload 必须与本机逐字节一致才允许刷入。新增机型只需加一个
conf 文件（填入该板原厂 mtd0 的 BL2/BL31 指纹）再编译。

备注：
1. PT1500-H10 的 DDR3 与 DDR4 真机mtd0BL2/BL31 完全一致，差异仅在 GPT、BL33与证书链，故两者共用 `pt1500.conf`。

## 内核模块

`modules/<series>.ko`（如 `5.10.ko`、`6.18.ko`）每个内核**系列**一份，按 `osrelease`
的 major.minor 选择，且仅在分区表不可写时加载。

一个系列共用一份：模块在这类构建里不带符号 CRC 表，`vermagic` 是被内核整串 `strcmp`
比对的，所以加载时会把 vermagic 里的 release 改写成本机 `osrelease`（与
`build-module.sh` 编译期改写的思路相同，只是挪到加载时）。同系列内 `struct module`、
`struct mtd_info` 布局一致，改写是安全的；长度对不上（例如 `6.18.55` 与 `6.18.5`）时
拒绝加载并提示为该系列补一份模块。

```sh
KDIR=<kernel tree> CROSS_COMPILE=aarch64-openwrt-linux-musl- \
TARGET_RELEASE=6.18.55 EXPECTED_MODULE_SIZE=1088 ./scripts/build-module.sh
# -> modules/6.18.ko
```

新增一个内核系列时：跑一次上面的命令，把产出的 `modules/<series>.ko` 提交进仓库，
再重新编译主程序即可内嵌。CI 不构建模块（需要目标内核树），只随提交更新。

## Build / 构建

```sh
./scripts/build.sh        # module (from KDIR) + program
```

Output: `target/aarch64-unknown-linux-musl/release/rtl9617c-migration-tools`,
statically linked.

## Tests / 测试

```sh
cargo test
```

## CI / 自动构建

| Workflow              | 触发方式            | 产出                                                          |
| --------------------- | ------------------- | ------------------------------------------------------------- |
| `.github/workflows/ci.yml`      | push / PR / 手动    | `cargo fmt`、`cargo clippy -D warnings`、`cargo test`、aarch64 musl 静态二进制（qemu 冒烟测试 + artifact） |
| `.github/workflows/release.yml` | 推送 tag `v*` / 手动 | GitHub Release 附件：静态二进制 + `sha256sums.txt`（手动触发时 tag 留空则取 `Cargo.toml` 的 version，如 `v0.1.0`，并在当前提交上自动建 tag） |
CI 固定使用 stable 工具链（edition 2024 需要 1.85+），交叉编译目标为
`aarch64-unknown-linux-musl`，链接器沿用 `.cargo/config.toml` 里的 `rust-lld`
（CI 会自动 `rustup component add rust-lld`），产物为静态链接，直接拷到设备上就能跑。

内核模块不进 CI、也不作为 Release 附件：它必需的 KDIR 是目标设备的内核树，
通用 runner 上没有；而且 `.ko` 最终被 `include_bytes!` 内嵌进主程序二进制，单独
发布没有意义。需要新系列时用 `scripts/build-module.sh` 本地产出并提交即可。
