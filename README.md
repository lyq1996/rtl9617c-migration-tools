# RTL9617C Migration Tools

适配机型：
- Tianyi TYFZA1 4GE （`tianyi-tyfza1-4ge`）
- Tianyi TYFZA1 2.5G （`tianyi-tyfza1-2p5g`）
- Youhua PT1500-H10-CT 2.5G（`youhua-pt1500-h10-ct-2p5g`）
- Youhua PT1570-H10-CT 2.5G（`youhua-pt1570-h10-ct-2p5g`）

## 使用

1. 上传 `rtl9617c-migration-tools` 到设备的 `/tmp` 目录：

```sh
chmod +x /tmp/rtl9617c-migration-tools
/tmp/rtl9617c-migration-tools
```

2. 打开 `http://<设备IP>:3333/`，下载需要保留的分区；
3. 上传本机型的 `u-boot.bin`：工具解析镜像，BL2/BL31 与本机 `mtd0` 一致才允许刷入；
4. 使用 [rtl9617c-factory](https://github.com/lyq1996/rtl9617c-factory) 组装 Factory 分区；
5. 重启后访问 `http://192.168.0.1/`，按U-Boot指引刷入固件。

完整流程：**备份 → 刷写 mtd0 → 重启**。

## 操作范围

只写 `boot`（mtd0）分区。`env`、`env2`、`Factory` 等其他分区保持原样。

## 备份

当前只提供物理分区下载。整机备份请随后进入 U-Boot 进行。

## 机型

```ini
name       = PT1500-H10-CT-2P5G
compatible = youhua,pt1500-h10-ct-2p5g
bl2        = <BL2 payload 的 SHA-256>
bl31       = <BL31 payload 的 SHA-256>
bl33       = <BL33 payload 的 SHA-256>
```

机型按原厂 `mtd0` 中 FIP 的 BL2、BL31、BL33 三个 payload 的 SHA-256 识别。
刷入时只校验上传镜像的BL2、BL31 与本机 `mtd0` 逐字节一致。
页面会显示识别出的机型，请选择对应硬件版本的 U-Boot 镜像。

## 内核模块

`modules/<release>.ko` 每个内核一份，按 `osrelease` 选择，且仅在分区表不可写时加载。

```sh
KDIR=<kernel tree> CROSS_COMPILE=aarch64-openwrt-linux-musl- \
TARGET_RELEASE=6.18.55 EXPECTED_MODULE_SIZE=1088 ./scripts/build-module.sh
```

提供6.18.55的目的是，当U-Boot无法使用时，可在6.18.55内核的OpenWrt下解锁mtd0强刷U-Boot。

## Build / 构建
生成 `modules/<release>.ko`，再执行：

```sh
./scripts/build.sh
```

Output: `target/aarch64-unknown-linux-musl/release/rtl9617c-migration-tools`,
statically linked.

## Tests / 测试

```sh
cargo test
```
