# RTL9617C Migration Tools

适配机型：

- Tianyi TYFZA1 4GE（`tianyi-tyfza1-4ge`）
- Tianyi TYFZA1 2.5G（`tianyi-tyfza1-2p5g`）
- Youhua PT1500-H10-CT 2.5G（`youhua-pt1500-h10-ct-2p5g`）
- Youhua PT1570-H10-CT 2.5G（`youhua-pt1570-h10-ct-2p5g`）
- Youhua PT1570-H10-CU 2.5G（`youhua-pt1570-h10-cu-2p5g`）
- Youhua PT1570-R10-CU 4GE（`youhua-pt1570-r10-cu-4ge`）

## 使用

1. 从 GitHub Release 下载 `rtl9617c-migration-tools`，上传到设备 `/tmp`：

```sh
chmod +x /tmp/rtl9617c-migration-tools
/tmp/rtl9617c-migration-tools
```

2. 打开 `http://<设备IP>:3333/`，下载需要保留的分区；
3. 上传本机型的 `u-boot.bin`：工具解析镜像，BL2/BL31 与本机 `mtd0` 一致才允许刷入；
4. 使用 [rtl9617c-factory](https://github.com/lyq1996/rtl9617c-factory) 组装 Factory 分区；
5. 重启后访问 `http://192.168.0.1/`，按 U-Boot 指引刷入固件。

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
刷入时只校验上传镜像的 BL2、BL31 与本机 `mtd0` 逐字节一致。
页面会显示识别出的机型，请选择对应硬件版本的 U-Boot 镜像。

## 内核模块

支持 5.10、6.18 两个内核 series。模块由 GitHub Actions 从对应 Linux 源码编译，仓库不保存预编译 `.ko`。

| series | 变体 | EXPECTED_MODULE_SIZE |
|---|---|---:|
| 5.10 | 5.10-832 | 832 |
| 5.10 | 5.10-896 | 896 |
| 6.18 | 6.18-704 | 704 |
| 6.18 | 6.18-1088 | 1088 |

运行时按 `osrelease` 的 series 选择模块，并在同 series 内按 `EXPECTED_MODULE_SIZE` 升序尝试；当前变体不匹配时自动 fallback 到下一个。

## 构建与发布

模块、测试和静态二进制均由 GitHub Actions 完成。推送 tag 时自动创建 GitHub Release 并上传 `rtl9617c-migration-tools`。

## 测试

```sh
cargo test
```
