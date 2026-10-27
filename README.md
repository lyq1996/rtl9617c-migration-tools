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
3. 上传本机型的 `u-boot.bin`；
4. 重启后访问`http://192.168.0.1/` 继续。

## 写入范围
只写 `boot` 分区

## 备份

只提供物理分区下载。`ubi_*` 是 `ubi_device` 内部的卷，系统占用时打不开，
读出来也不是可恢复的镜像；整机备份请下 `ubi_device`或随后进入uboot下载。

## 机型
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
