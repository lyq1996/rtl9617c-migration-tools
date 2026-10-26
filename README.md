# RTL9617C Migration Tools
适配机型：
1. PT1500-H10；
2. TYFZA1；

## 使用
1. 上传`rtl9617c-migration-tools`到设备的`/tmp`目录

```sh
chmod +x /tmp/rtl9617c-migration-tools
/tmp/rtl9617c-migration-tools
```
2. 打开 `http://<设备IP>:3333/`，下载备份；
3. 上传本机型的 `u-boot.bin`；
4. 重启后访问`http://192.168.0.1/` 继续。

## 写入范围
只写 `boot` 分区

`boot` 是 2 MiB GPT 磁盘，两份相同 FIP 在 0x40000 与 0x100000。
工具只把新 U-Boot 补进设备自己的 FIP，BL2、BL31、证书链和两份 FIP 之外的字节都不动；
BL33 可用空间即原厂 U-Boot 的大小，放不下的镜像会被拒绝。

先写 FIP1 并读回，再写 FIP0；中断时主份完好。擦除前会把当前 boot 区存到
`/tmp/rtl9617c-boot-backup.bin` 并提供下载。

## 备份

只提供物理分区下载。`ubi_*` 是 `ubi_device` 内部的卷，系统占用时打不开，
读出来也不是可恢复的镜像；整机备份请下 `ubi_device`。

## 机型

Each product is one file in `boards/`, compiled in by `build.rs`:

```ini
name       = PT1500-H10
compatible = youhua,pt1500
volumes    = ubi_Config, ubi_DTB0, ubi_k0, ubi_r0, ...
```

`volumes` 取自 `layout.c`，按卷 id 顺序，用于识别机型；`compatible` 用于校验上传的
`u-boot.bin`。新增机型只需加一个文件再编译。

## 内核模块

`modules/<release>.ko` 每个内核一份，按 `osrelease` 选择，且仅在分区表不可写时加载。

```sh
KDIR=<kernel tree> CROSS_COMPILE=aarch64-openwrt-linux-musl- \
TARGET_RELEASE=6.18.55 EXPECTED_MODULE_SIZE=1088 ./scripts/build-module.sh
```

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
