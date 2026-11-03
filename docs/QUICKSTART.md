# PW2 快速安装

适用 **Paperwhite 2 / B0D4 / 固件 5.12.2.1.1**，已安装能运行的 KUAL。原厂设备先按[完整记录的越狱和 KUAL 部分](PW2_REPRODUCTION.md#1-从原厂设备开始)操作。全程先保持飞行模式。已经安装本项目的设备请走[升级或维护](../device/install/README.md)，首次安装按钮会拒绝覆盖它。

| 目标 | KUAL 可用后的 USB 往返 |
| --- | --- |
| 只记录到 Kindle | 复制候选包并离线运行一次。需要查看诊断时才接回 |
| 实验性直传，已经有精确书籍对应 | 初装并检查重启、生成试传预览并在 Mac 核对、提交试传。最后一次拔线后可在手机核对 |
| 实验性直传，初次还没有对应 | 先在初装回读时一起取得书籍来源键，下一轮同时导入映射并生成预览 |

**计数口径**：一轮指接 USB 准备文件、安全弹出、在 Kindle 上连续完成该轮操作。已有 KUAL 但无兼容性探针时，首次同步共需探针、初装、预览、确认四轮；已有探针时三轮。越狱、Hotfix 和 KUAL 安装另计。首次交接前还须有一次通过的兼容性探针，可并入 KUAL 安装后的操作。若 KUAL 已装但没有探针，在首次复制候选包后离线点 `Export root compatibility probe`，接回后直接进入第一轮。遇到故障才单独导出排查。新合并入口已做离线流程测试，尚未在另一台全新 PW2 上跑完这条路线。实际入账范围见[实机记录](PW2_REPRODUCTION.md#4-实机结果和复核方法)。

## 准备候选包

从 [GitHub Releases](https://github.com/WanhaoZhang/kindle-device-autosync/releases) 取得 PW2 预发布包与旁边的 SHA256 文件。也可以按[源码构建步骤](PW2_REPRODUCTION.md#2-从源码构建与核对)自己构建。使用与包中 `SOURCE.txt` 一致的源码版本运行 Mac 助手。

0.2.0 完整包同时包含 `kindle-native-sync-package/` 和 `mac-assistant/`。Mac 运行助手只需 Python 3.11+，使用预编译 ARM 包可省去本机安装 Rust 和交叉工具链。

macOS 在下载目录执行以下命令。这里以发布包文件名和默认 Kindle 挂载点为例。

```sh
shasum -a 256 -c kindle-pw2-preview-0.2.0.zip.sha256
unzip kindle-pw2-preview-0.2.0.zip -d kindle-pw2-preview
cd kindle-pw2-preview/kindle-native-sync-package
shasum -a 256 -c SHA256SUMS
```

首次复制时，确认目标目录不存在，再执行：

```sh
mount_dir=/Volumes/Kindle
test -d "$mount_dir/documents" &&
test ! -e "$mount_dir/kindle-native-sync-package" &&
test ! -e "$mount_dir/extensions/kindle-native-sync" &&
mkdir -p "$mount_dir/extensions" &&
cp -R . "$mount_dir/kindle-native-sync-package" &&
cp -R kual "$mount_dir/extensions/kindle-native-sync"
```

## 只采集

安全弹出、拔线，在同一次离线操作中打开 **KUAL → Native Reading Sync**，先点 `Export root compatibility probe` 和 `Run local candidate self-test (no network)`，再点 `Install / upgrade (collection only)`。安装器会重新核对设备、空间、哈希和 ELF 并启动服务。正常新读一本书，回书库约 30 秒后可点 `Export sanitized status`。若要验收，接回一次集中查看 `probe.json`、`selftest.json`、`install-result.json`、`status.json`。在设备目录 `.kindle-native-sync-export/` 下查找，Finder 可按 **⌘⇧.** 显示隐藏目录。

此路线安装原始事件采集。设备端按书解析需要后续导入配置。日常读完回书库即可，原生 `;dm` 仅用于对照诊断。

## 试用 Kindle 直传

### 第一轮 · 安装、导入、保护和重启

1. 候选包复制后保持 Kindle 接在 Mac。进入完整包中的 `mac-assistant/` 运行 `sh setup.sh`、`sh 启动设备配置.command`，打开 **http://127.0.0.1:8766/**。扫码建立 Kindle 专用身份，按[配置与交接](CONFIGURATION.md)选择真正的旧实例或新设备初始化，确认交接，导出 USB 配置。第一次没有映射时用空列表。
2. 安全弹出并拔线，保持飞行模式。打开 KUAL → **Native Reading Sync**，点一次 **First install + import + protect + reboot**。它执行探针、安装、安装后自检、配置导入、OTA 防护和重启。重启后保持飞行模式。
3. 原生读一本书 15–40 秒并翻几页，退出到书库约 30 秒，回 KUAL 点 **Check protected reboot + export status**。接回一次，查 `setup-result.json`、`ota-status.json`、`status.json`。只在 `ready_after_reboot: true` 且根分区为 `read_only` 时进入联网轮。诊断缺失或失败时按[排错表](PW2_REPRODUCTION.md#5-实际遇到的故障)定位。

### 第二轮 · 用同一次连接处理映射和试传预览

1. 若已有对应关系，使用助手中已核对的 `local_book_id`。若空映射起步，点“读取已解析书籍”，搜索微信读书的准确版本，保存并导出更新配置。如果没有书籍键，先确认读到完整关书窗口且 `status.json` 已刷新。
2. 在 Mac 助手目录生成准备请求。命令会取更新配置的绑定，或者使用刚导出状态中的绑定；用实际精确键替换占位符。

   ```sh
   python3 stage_trial.py prepare --book 'EXACT_LOCAL_BOOK_ID' --accept-unverified-performance
   ```

3. 安全弹出拔线。若本轮更新了映射，先从 KUAL 点 **Import USB configuration**。按上一轮结果确认 OTA 防护后，手动连接 Wi‑Fi。若第一轮已经形成同一本书 1–60 秒的完整计时窗口，可以复用该新增窗口；没有完整短窗口时，在本轮读约 15–40 秒并翻页，退到书库等约 30 秒。打开 KUAL 点 **TLS check + authorize + prepare trial**。联网探针和预览会写报告，时长提交在下一轮。
4. 接回 Mac，查看 `.kindle-native-sync-export/action-authorize-prepare.json`。只有 `state: prepared` 才有可确认的 `plan`。核对其中的书籍版本、日期、真实完整窗口、小时分配和秒数。

### 第三轮 · 确认试传

在 Mac 助手目录生成确认请求，替换成刚核对的 `plan.plan_id`：

```sh
python3 stage_trial.py confirm --plan-id 'EXACT_REVIEWED_PLAN_ID' --accept-unverified-protocol
```

安全弹出拔线，Kindle 保持联网、醒着、阅读器已退出，点一次 **Confirm approved trial + enable if verified**。只有微信读书的单书累计、目标日期统计和原阅读位置全部核验通过，才会开启实验性自动同步，并尝试排空合格积压。可以先在手机查看两处时长。若需检查程序判定，在下次接 USB 时读取 `action-confirm-enable.json`；`experimental_activation.automatic_upload_enabled: true` 表示开关已启用，`automatic_drain` 是后续批次结果。

平时读完回书库并保持 Wi-Fi、屏幕醒着，设备会在明确空闲后尝试同步。断网时保存在 Kindle。遇到 `needs_review` 或 `unknown`，保留原账本；已发送批次只能按[只读核验流程](PW2_REPRODUCTION.md#4-实机结果和复核方法)处理。

## 日后批量新增书籍 · 一轮 USB

前提是设备已安装 0.2.0+、已有可用账号和一次设备状态导出。

1. 多本新书放进 Kindle 后，保持同一次 USB 连接，打开配置助手。
2. 在页面顶部逐本选择 Kindle 候选、搜索微信读书版本、勾选核对并保存。
3. 全部选好后，点“导出候选对应到 Kindle”。
4. 安全弹出拔线，KUAL 点 **Import selected books + export status**。之后照常读书。新书尚未读过时，程序在首次真实阅读、关书并核验来源后启用对应。

无需为了登记每本新书逐本试读并接回。要看导入结果时，可在下次正常连接查看 `.kindle-native-sync-export/book-candidate-import.json`。旧版设备先升级；原有基础对应的变更仍按维护流程核验。

若短试传因为统计延迟而待核验，可在设备保持联网、醒着和空闲时，再点原来的 **Confirm approved trial + enable if verified**。本版对已经提交的同一计划只做只读核验；结果未知时保留原账本并拒绝重放。这个步骤可以在同一次拔线期间完成。
