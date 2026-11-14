# RTL9617C Migration Tools：CI 构建与多版本内核模块方案

## 1. 最终结论

- 仓库不再保存 `modules/*.ko`，内核模块全部由 GitHub Actions 编译。
- 只按内核 series 选择：`5.10`、`6.18`。
- 同一 series 可以有多个 `EXPECTED_MODULE_SIZE` 版本；加载时按顺序尝试，失败自动 fallback。
- 初始版本：

| series | 变体 | EXPECTED_MODULE_SIZE |
|---|---|---:|
| 5.10 | 5.10-832 | 832 |
| 5.10 | 5.10-896 | 896 |
| 6.18 | 6.18-704 | 704 |
| 6.18 | 6.18-1088（HEAD） | 1088 |

- 源码使用当前最新 longterm：`5.10.274`、`6.18.63`。
- 接受运行时改写 `vermagic`。
- 测试 fixture 保留。
- tag 构建自动创建 GitHub Release。
- 不提供本地模块编译流程；开发者自行准备工具链，仓库不再提供 `fetch-kernel.sh` 的本地用法。

## 2. CI 文件布局

所有 CI 相关文件放在 `.github/workflows/`：

```text
.github/workflows/
├── build.yml
├── build-module.sh
├── kernels.json
├── config-5.10-832
├── config-5.10-896
├── config-6.18-704
└── config-6.18-1088
```

`kernels.json` 采用扁平列表，一个条目对应一个 .ko：

```json
[
  {
    "series": "5.10",
    "variant": "832",
    "version": "5.10.274",
    "url": "https://cdn.kernel.org/pub/linux/kernel/v5.x/linux-5.10.274.tar.xz",
    "sha256": "9f87a8b523560ca7740ea4a149c6dc0dab7814c2a41bccdb739b291a7e120b8c",
    "config": ".github/workflows/config-5.10-832",
    "expected_module_size": 832
  },
  {
    "series": "5.10",
    "variant": "896",
    "version": "5.10.274",
    "url": "https://cdn.kernel.org/pub/linux/kernel/v5.x/linux-5.10.274.tar.xz",
    "sha256": "9f87a8b523560ca7740ea4a149c6dc0dab7814c2a41bccdb739b291a7e120b8c",
    "config": ".github/workflows/config-5.10-896",
    "expected_module_size": 896
  },
  {
    "series": "6.18",
    "variant": "704",
    "version": "6.18.63",
    "url": "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.18.63.tar.xz",
    "sha256": "81764a545072d160748beff4d9de2e7d95aa9f16e8261110288f067a2cc61d71",
    "config": ".github/workflows/config-6.18-704",
    "expected_module_size": 704
  },
  {
    "series": "6.18",
    "variant": "1088",
    "version": "6.18.63",
    "url": "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.18.63.tar.xz",
    "sha256": "81764a545072d160748beff4d9de2e7d95aa9f16e8261110288f067a2cc61d71",
    "config": ".github/workflows/config-6.18-1088",
    "expected_module_size": 1088
  }
]
```

新增 series 或 size 变体只需加一条记录和一个配置片段，workflow 不改。

## 3. 配置片段

`config-5.10-832`：

```text
# CONFIG_PREEMPT is not set
CONFIG_PREEMPT_NONE=y
# CONFIG_FTRACE is not set
# CONFIG_LOCALVERSION_AUTO is not set
CONFIG_LOCALVERSION="-rtl9617c-module-slot-0123456789abcdef"
```

`config-5.10-896`：

```text
# CONFIG_PREEMPT is not set
CONFIG_PREEMPT_NONE=y
CONFIG_FTRACE=y
# CONFIG_LOCALVERSION_AUTO is not set
CONFIG_LOCALVERSION="-rtl9617c-module-slot-0123456789abcdef"
```

`config-6.18-704`：

```text
# CONFIG_CORESIGHT is not set
# CONFIG_ARM_CORESIGHT_PMU_ARCH_SYSTEM_PMU is not set
# CONFIG_NVIDIA_CORESIGHT_PMU_ARCH_SYSTEM_PMU is not set
# CONFIG_PROFILING is not set
# CONFIG_PERF_EVENTS is not set
# CONFIG_PREEMPT is not set
CONFIG_PREEMPT_NONE=y
# CONFIG_KALLSYMS_ALL is not set
# CONFIG_LOCALVERSION_AUTO is not set
CONFIG_LOCALVERSION="-rtl9617c-module-slot-0123456789abcdef"
```

`config-6.18-1088`：

```text
# CONFIG_PREEMPT is not set
CONFIG_PREEMPT_NONE=y
# CONFIG_LOCALVERSION_AUTO is not set
CONFIG_LOCALVERSION="-rtl9617c-module-slot-0123456789abcdef"
```

## 4. CI 流程

`build.yml`：

1. `prepare` 用 `jq -c` 读取 `kernels.json`，输出矩阵。
2. `module` 矩阵调用 `.github/workflows/build-module.sh`：
   - 下载并校验内核 tarball，缓存到 `~/.cache/rtl9617c-kernels`；
   - 解压、`defconfig`、应用配置片段、`olddefconfig`、`modules_prepare`；
   - 编译模块，校验 AArch64、vermagic series、`this_module` 大小、未定义符号；
   - 输出 `modules/<series>-<variant>.ko` 并上传 artifact。
3. `tool` 下载全部模块到 `modules/`，执行：
   - `RTL9617C_REQUIRE_MODULES=1 cargo test --locked`
   - `RTL9617C_REQUIRE_MODULES=1 cargo build --release --locked --target aarch64-unknown-linux-musl`
   - 上传静态二进制。
4. tag 触发 `release`，创建 GitHub Release 并上传二进制；该 job 需要 `contents: write`。

依赖：`gcc-aarch64-linux-gnu`、`binutils-aarch64-linux-gnu`、`bc`、`bison`、`flex`、`libssl-dev`、`libelf-dev`、`xz-utils`、`jq`。

## 5. 运行时加载

模块文件名：`modules/5.10-832.ko`、`modules/5.10-896.ko`、`modules/6.18-704.ko`、`modules/6.18-1088.ko`。

`build.rs`：

- 扫描 `modules/*.ko`；
- 文件名解析为 `series` 和 `variant`；
- 允许同一 series 多个文件，禁止同一 `series-variant` 重复；
- 按 series、variant 数字升序排序，保证 `6.18-704` 在 `6.18-1088` 之前；
- 生成 `MODULES: &[(&str, &str, &[u8])]`，依次为 series、variant、bytes；同一 series 可出现多次；
- `modules/` 为空时生成空表并警告；`RTL9617C_REQUIRE_MODULES=1` 时直接失败。

`src/unlock.rs`：

- `kernel_series(release)`：取前两段数字，例如 `6.18.63` → `6.18`。
- `modules(series)`：返回该 series 的所有候选模块。
- `load(running_release)`：
  1. 取 series；
  2. 按 build.rs 生成顺序（size 升序）尝试候选模块；
  3. 每个候选先校验 vermagic series，再在内存中把 `vermagic` 的 release 改写为 `running_release`，flags 保持不变；
  4. 调用 `init_module`；
  5. `EEXIST` 视为成功；`ENOEXEC` 视为该 size 不匹配，继续下一个；其他错误直接返回；
  6. 全部失败时返回已尝试的 variant 列表。

vermagic 改写：

- 使用编译时的长 `CONFIG_LOCALVERSION` 占位，保证运行 release 更短；
- 找到 `vermagic=`，按第一个空格拆出 flags；
- 写入 `vermagic=<running_release> <flags>\0`，旧值剩余字节补 0；
- 不改变文件长度，不移动后续 section。

## 6. 代码改动

- `build.rs`：按上面的文件名规则生成多候选表。
- `src/unlock.rs`：series 选择、多候选 fallback、vermagic 改写。
- `src/main.rs`：启动报告改为 series + variants。
- `src/unlock.rs` 测试：series 解析、vermagic 改写、fallback 顺序、空模块跳过。
- `.gitignore`：增加 `/modules/*.ko`。
- 删除 `modules/5.10.70.ko`、`modules/6.18.55.ko`。
- 删除 `scripts/build-module.sh`；`scripts/build.sh` 不再调用它。

## 7. README 最终内容

README 只保留最终状态：

- 保留现有使用步骤、机型和分区说明；
- 支持 5.10、6.18；
- 模块由 GitHub Actions 构建，仓库不保存预编译 .ko；
- 同一 series 可带多个 size 变体，加载时自动 fallback；
- 发布物为静态 `rtl9617c-migration-tools`；
- 不再写本地模块编译流程、旧版 1088 命令、改造记录或中间状态。

## 8. 验证

- CI 产出 `5.10-832.ko`、`5.10-896.ko`、`6.18-704.ko`、`6.18-1088.ko`。
- `cargo test` 通过。
- 5.10 标准 896 设备先尝试 832，失败后 fallback 到 896。
- 6.18 设备先尝试 704，失败则 fallback 到 1088。
- 故意改错 `expected_module_size` 时 CI 失败。
