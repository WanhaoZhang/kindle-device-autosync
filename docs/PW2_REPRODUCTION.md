# PW2 完整复现和排错记录

本记录覆盖 Mac 网页导入、PW2 越狱后的安装、采集和直传验证。设备实验基线为 [2bd766d](https://github.com/WanhaoZhang/kindle-device-autosync/tree/2bd766d77dbdd73eb5a6de1042cd5bfe3c7d4118)。快速入口见[快速安装](QUICKSTART.md)。时间采用 Asia/Shanghai，样本以秒数和行为公开，真实书名、身份、原始日志、二维码和数据库保留在原设备或本机。

## 1. 从原厂设备开始

1. **型号与备份。** 设置 → 设备选项 → 设备信息，确认 Paperwhite 2、固件 **5.12.2.1.1**，项目支持序列号前四位 **B0D4**。充电、开飞行模式，备份 USB 用户区及原浏览器文件。避免在该过程中安装其他固件。
2. **KindleBreak。** 从原作者帖下载 `jb-kindlebreak.zip`，记录中的 MD5 为 `0215c36cc1e3ad8136a67daebe369452`。解压得到 `jb`、`jb.sh`、`kindlebreak.html`、`kindlebreak.jxr`，直接放 USB 根目录，与 `documents/` 同层。安全弹出拔线，在实验浏览器打开 `file:///mnt/us/kindlebreak.html`，等待自动重启。浏览器冻结或提示 Application Error 曾是正常触发过程的一部分。接回检查根目录 `kindlebreak_log.txt` 是否记录安装开发者密钥并重启。原实验曾另备份和预置浏览器 LocalStorage，原作者标准路径不要求这一步，应先走标准流程。[KindleBreak 原作者说明](https://www.mobileread.com/forums/showthread.php?t=338268)
3. **Legacy hotfix。** 从 NiLuJe 发布页取下表中的压缩包，解压只将 `Update_jailbreak_hotfix_1.16.N_install.bin` 放 USB 根目录。安全弹出后保持飞行模式，在设置菜单点 **更新您的 Kindle**，确认并等待重启。更新项灰色或报错时记录提示、复核越狱和包版本。
4. **MRPI 与 KUAL。** 接回后解压 MRPI，将其中的 `extensions/MRInstaller/` 和 `mrpackages/` 放到 USB 对应目录。将 KUAL coplate 中的 `Update_KUALBooklet_c6ac782_install.bin` 放入 `mrpackages/`。同次连接可把本项目候选包及 KUAL 扩展复制好。安全弹出拔线，在首页搜索半角 `;log mrpi`。等待安装并在书库打开 **KUAL → Native Reading Sync**。装好 KUAL 就运行本项目的 `Export root compatibility probe`，接回时一起读取探针和 MRPI 日志。Hotfix、MRPI/KUAL 使用各自兼容的包，具体版本从同一发布源核对。[NiLuJe 发布页](https://www.mobileread.com/forums/showthread.php?t=225030)

| 本次使用的原始归档 | SHA256 |
| --- | --- |
| `JailBreak-1.16.N-FW-5.x-hotfix.zip` | `2bd8e0a4129fd7812eb480f71fba0e48333cd21676d2f772e78b16968bbf9eac` |
| `kual-mrinstaller-1.7.N-r19303.tar.xz` | `36b3c28a44ad1a91609cc82d2bb9836ad6d7125ba1f63596307bf93f17a9255e` |
| `KUAL-c6ac782-20250419.tar.xz` | `8dace9db8ff7b7de80662a48e830296990e978efc71c281efde43b370bc7afe3` |

保持飞行模式直到本项目 OTA 防护完成并核验重启。热修复保留越狱环境。防护动作和恢复条件另见 [OTA 文档](../device/ota/README.md)。

## 2. 从源码构建与核对

Mac 配置助手需要 Python 3.11+。固件上跑 C 采集器、C LIPC 适配器和 Rust worker。候选 ARM 包采用实际试验过的 Zig **0.13.0** 静态构建路径，NiLuJe `kindlepw2` GCC 是另一个受支持的构建配置。自行从原作者安装 Rust 和 Zig。下载 Zig 时核对其发布页列出的对应平台校验值。[Zig 下载](https://ziglang.org/download/) · [NiLuJe 工具链](https://github.com/NiLuJe/koxtoolchain)

```sh
git clone https://github.com/WanhaoZhang/kindle-device-autosync.git
cd kindle-device-autosync
rustup toolchain install 1.91.0 --profile minimal
rustup target add armv7-unknown-linux-musleabihf --toolchain 1.91.0
KSYNC_ZIG=/absolute/path/to/zig KSYNC_BUILD_DIR=device/build/out ./device/build/build.sh --experimental-static-musl
(cd device/build/out/package && shasum -a 256 -c SHA256SUMS)
file device/build/out/package/bin/*
```

`device/build/out/package` 是输出目录，新建构建输出路径可避免覆盖旧包。确认 `manifest.json` 的提交、源码 SHA、三个 ARM32 静态 ELF 与构建 profile。部署结构为 `/mnt/us/kindle-native-sync-package/` 和 `/mnt/us/extensions/kindle-native-sync/`。构建过程、上游版本和许可见[安装说明](../device/install/README.md)与[第三方声明](../THIRD_PARTY_NOTICES.md)。发布 ZIP 是同一脚本生成的实验包。

## 3. 分步复现实机实验

1. **离线准入。** 依次点 KUAL 的 `Export root compatibility probe`、`Run local candidate self-test (no network)`。首次实机为 Linux 3.0.35-lab126、glibc 2.20、Upstart/LIPC 可用、真实根只读。私有分区总量约 62 MiB，初次可用约 41 MiB。回读 `hardware_supported`、`installation_supported`、`private_filesystem_ok`。所有候选程序都应从这台设备的实际环境重新检查。
2. **只采集对照。** 安装 `Install / upgrade (collection only)`，保持离线。读 2–3 分钟、翻页、退出书籍约 30 秒，合盖约 20 秒再唤醒。可在同次离线操作导出 `Export sanitized status` 并执行一次 `;dm`，等待约 30 秒后接回。将设备封存事件和官方日志的开关书、计数区间、日期和秒数交叉核对。`;dm` 属于研究对照。发生卡顿时，同一本书比较启用与停用后的翻页、打开和退出，再看 `service-probe.json`。[正式验收矩阵](DEVICE_VALIDATION.md)
3. **配置与交接。** 开 Mac 助手，选择实际旧项目目录或新初始化，独立扫码、精确匹配、导出并导入。原 Mac 写入被封锁，原数据留档。未匹配时先导入空映射并完成一个真实关书窗口，通过设备状态查精确来源键。[配置与交接](CONFIGURATION.md)
4. **防护重启。** 首次联合入口用 `First install + import + protect + reboot`。本次实机当时已有未配置、已停用的旧预览，走 `Setup stopped preview + import + reboot`，遇中断则先恢复事务，再继续。重启后 `Check protected reboot + export status`。分别检查保护后的新启动 ID、两份 OTA 原件备份哈希及真实根只读。核验前保持飞行模式。
5. **试传。** 已验证 OTA 保护后才手动连 Wi-Fi。真实 TLS 自检检验证书链、主机名、DNS 和文件系统，授权后预览新增短会话。Mac 核对确切 `plan_id` 后才确认。USB 暂存文件和 KUAL 菜单的用法见[快速安装](QUICKSTART.md#试用-kindle-直传)。软件保留关书同秒计数、原始小时分配和日期，提交前持久登记，提交后回读书籍、日期与位置。
6. **延迟核验与后续自动运行。** 实机短试传先增加日期统计，书籍累计后来也精确增长。原计划在旧 worker 下已提交一次，后续只读核验；经过严格的已知二进制与协议源码校验后，新的 runtime 接纳原回执。随后后台确实发出了历史日期批次。这个旧版接纳仅用于特定已知构建和原账本，正常新安装使用本机试传流程。[Worker 说明](../device/worker/README.md#显式实验性自动使用)

## 4. 实机结果和复核方法

| 观测样本 | 服务端与程序结果 |
| --- | --- |
| 同日真实 4 秒试传 | 日期先 +4，书籍累计后 +4，原位置相同。只读复核后确认 |
| 之前一天合并 546 秒，来自 4 个窗口 | 三个真实小时桶分别 283、157、106 秒。收到成功回执，8 轮回读后书籍和目标日期仍为 +0，观察截止时仍待核验 |
| 当日剩余 188 秒 | 保存在 Kindle，受待核验队列门禁保护，未发送 |

离线对照采用四个相同时长、相同小时的窗口和**合成**账号、设备、令牌、发送时间、随机值，Rust 与 MiuRead Lua 的请求字段及两层签名一致。该比较检验本地协议移植，服务端跨日、多小时、连续批次行为仍待真实验证。原账号旧 Mac 试验另有待核验记录，保存在原账本。[上游协议实测边界](https://github.com/miumiupy98-art/miuread-koreader/blob/ee9d6f24768349d428230036087c379dd957b30c/docs/offline-readtime-protocol-audit.md#L96-L104) · [对照的上游发送实现](https://github.com/miumiupy98-art/miuread-koreader/blob/c1893562aa25a870b8dd076d7be062a3348f823e/miuread.koplugin/miuread/eink_readtime.lua#L39-L51)

复核时先看对应日期及单书累计。需要程序级核验且具备已验证的 Kindle root 操作环境时，在设备已醒、已联网、空闲的状态下运行：

```sh
root=/var/local/kindle-native-sync
"$root/bin/ksync-worker" --root "$root" reconcile --plan-id EXACT_ACKNOWLEDGED_PLAN_ID
```

此路径只核对已明确收到成功回执的原批次，验证三项统计后更新本地账本。认证过期时可能续期。连接丢失或服务器状态不明的提交保持原状态。保留原计划与账本，避免以拆批、换日、删账本或换账号身份的方式重试。

## 5. 实际遇到的故障

| 症状 | 定位和处理 |
| --- | --- |
| 安装回执报错或根分区仍可写 | 检查 `/proc/mounts` 真实根挂载，忽略 `rootfs` 伪条目。旧固件 `mntroot` 退出码不够，当前代码核验内核结果并在必要时普通只读 remount。见 [235edca](https://github.com/WanhaoZhang/kindle-device-autosync/commit/235edca)、[47a0b9b](https://github.com/WanhaoZhang/kindle-device-autosync/commit/47a0b9b) |
| 升级后没有重启 | 先看 `setup-result.json` 的 `stage`，区分切换、导入、OTA、重启。保留 `.preview-upgrade` 与 USB 恢复副本，走 `Recover interrupted preview (keep disabled)`。导入或防护阶段失败时先定位，确认原安装完整后走 `Resume imported setup + protect + reboot` |
| 重启后私有目录权限报错 | PW2 启动过程可能将组权限扩展。当前启动修复只对项目拥有的已验证文件类型收紧权限，恢复 FIFO/状态通道；凭据留私有目录。见 [fd7435a](https://github.com/WanhaoZhang/kindle-device-autosync/commit/fd7435a) |
| 关书预览少算尾部 | 关书同秒的最后计数需一起归组；只迁移来源一致且从未提交的 pending 窗口。见 [b49f43e](https://github.com/WanhaoZhang/kindle-device-autosync/commit/b49f43e) |
| 偶发 `worker already running` | 回到书库等待现有 worker 退出。维护器仅对能证明操作尚未开始的锁冲突有限等待；已提交、未知或其他错误保持原回执 |
| 报日志缺口或配额不足 | 原生日志轮换的索引文件与真正数据区分。真实断档需要比对不可变事件和 sidecar；空间预算有硬门禁。见 [d85bf7d](https://github.com/WanhaoZhang/kindle-device-autosync/commit/d85bf7d)、[2bd766d](https://github.com/WanhaoZhang/kindle-device-autosync/commit/2bd766d) |
| 续期报错 | 当前续期不把过期 access token 作为续期凭据，核对同账号后原子保存。见 [2e95566](https://github.com/WanhaoZhang/kindle-device-autosync/commit/2e95566) |
| 已显示启用但没有新增时长 | 核对 `action-confirm-enable.json`、`status.json` 与账本回执，区分未发、待核验、已确认。成功回执与入账分别记录。过早读取状态可能只看到了上次导出 |
| 翻页或开关书明显变慢 | 用同书及相同条件执行 `Disable collection` 对照，并检查服务树。单台实机曾主观报告停用后稍有改善，未建立因果。翻页 P95、长期耗电和一周持续验证仍待完成 |

原实验中的 SETUP v2/v3、重复读短样本、重复制包、导出私有审计、分段权限修复、先提交再接纳旧回执，都是定位旧构建故障时用到的步骤。新安装按快速路线执行即可。日常维护使用 KUAL 的停用、状态导出与卸载；卸载保留原账本。恢复 OTA 原件需按[防护维护说明](../device/ota/README.md)单独执行。

普通状态导出也可能包含书名和来源键，公开 Issue 仅贴手工筛选后的字段。私有诊断包含原始记录，留在本机。开发者可从源码运行 `python3 -m unittest discover -v`、`make -C device/collector test`、`cargo +1.91.0 test --locked --manifest-path device/worker/Cargo.toml`，离线复现故障边界。
