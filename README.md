# Kindle Device Autosync

在 Kindle 上保留原生阅读器，通过小型 C 采集器记录阅读事件，再按需启动 Rust worker 解析每书、每日模型用时，并研究直接同步微信读书。

**这是 Kindle 设备端项目，处于实验预览阶段，默认只采集。** 已在一台 Paperwhite 2 / 固件 5.12.2.1.1 上完成 root 环境及 ARM worker 的无网络文件/SQLite 自检；常驻安装、休眠/USB循环、联网 TLS、真实性能、独立时长入账及一周稳定性验收尚未完成。不能将编译或自检通过理解为自动同步已经可用。

## 与网页版的区别

| 项目 | 运行位置与用途 |
| --- | --- |
| 本仓库 [kindle-device-autosync](https://github.com/WanhaoZhang/kindle-device-autosync) | Kindle 常驻采集、按需解析与直传；附带首次设置和维护用的 Mac 配置助手 |
| [kindle-native-reading-sync](https://github.com/WanhaoZhang/kindle-native-reading-sync) | Mac 网页版：USB 导入 Kindle 日志、查看每日统计、人工核对后同步 |

本仓库不包含网页版的日志导入、统计报告和 Mac 上传页面。首次配置助手用于独立扫码、书籍映射、上传权交接和读取导出的诊断，不在 Mac 提交阅读时长。两个仓库可分别克隆、运行和维护。拆分来源见 [仓库拆分说明](docs/REPOSITORY_SPLIT.md)。

## 运行方式

```text
Kindle 原生阅读 → C 增量采集并封存 → 明确关书后空闲至少 20 秒
                                      ↓
                        设备醒着、非 USB 导出时按需启动 worker
                                      ↓
                      通过验收且已有网络时才允许上报并回查
```

程序不主动开启 Wi-Fi、不定时唤醒、不申请阻止休眠的锁。读书时不因长时间停留在一页而判断“空闲”；合盖时短小落盘，未完成工作留待自然唤醒。未知或没有准确匹配的书籍继续保留，不能按相似标题猜测版本。

## 支持范围与准备

- 当前脚本仅允许 **PW2 / 型号前缀 B0D4 / 固件 5.12.2.1.1 / ARMv7**，其他设备会被拒绝。
- 需要用户另行准备、核验的 root / jailbreak、Upstart、LIPC、KUAL 环境；本仓库不自动越狱或升级固件。
- Mac 配置助手需要 Python **3.11+**。Rust **1.91.0** 和交叉工具链仅在构建设备程序时需要。
- 尚无经过全部真机验收的稳定安装包。生成候选包时必须核对 ARM ELF、来源和 SHA256；不要将 Mac 可执行文件复制成 Kindle 程序。

## 启动 Mac 配置助手

```sh
git clone https://github.com/WanhaoZhang/kindle-device-autosync.git
cd kindle-device-autosync
./setup.sh
./启动设备配置.command
```

浏览器访问 **http://127.0.0.1:8766/**。不要直接双击 `config-web/index.html`；页面依赖本地服务。助手默认仅在本机监听，开始登录前不会请求微信读书。

如果以前使用过网页版，必须选择**实际保存原凭据和上传账本的项目目录**，例如：

```sh
.venv-bridge/bin/python device_app.py --legacy-project "/absolute/path/to/your-existing-web-project" --open-browser
```

不要指向刚克隆的空目录来代替旧实例，也不要将旧 `.weread/` 复制进本仓库后重新开始。交接会在原项目保存上传封锁标记，保留原账本与待核验记录。纯新用户必须明确确认没有旧上传实例后，才能建立新的设备绑定。完整操作见 [首次配置与交接](docs/CONFIGURATION.md)。

## 构建并安装采集候选

默认 C 工具链为 NiLuJe `kindlepw2`，Rust worker 使用 ARM hard-float musl 静态目标，SQLite 的 C 工具链必须匹配。工具缺失时构建会停止，不会拿主机二进制充数。

```sh
rustup toolchain install 1.91.0 --profile minimal
rustup target add armv7-unknown-linux-musleabihf --toolchain 1.91.0
./device/build/build.sh
```

工具链变量、Zig 0.13.0 实验性替代路径、包校验与完整安装步骤见 [设备安装说明](device/install/README.md)。`experimental-static-musl` 仅代表另一种 ARM 候选构建，不能宣称通过 NiLuJe 工具链兼容认证。

候选包放入 Kindle USB 根目录的 `kindle-native-sync-package/`，其 `kual/` 内容放入 `extensions/kindle-native-sync/`。安全弹出并拔线后，从 KUAL 的 **Native Reading Sync** 菜单依次进行：

1. `Export root compatibility probe`，接回核对型号、固件、空间及 root 状态。
2. `Run local candidate self-test (no network)`，先验证程序实际能运行。
3. `Install / upgrade (collection only)`，安装默认只采集服务。
4. 用原生阅读器新读一本书，关书后等 30 秒；合盖、唤醒，再选择 `Export sanitized status`，接回助手核对采集及服务状态。

初期保持飞行模式。首次联网前另行核验适合本固件的可逆 OTA 防护；旧版 jailbreak hotfix 本身不能当作防升级证明。不要为安装本项目升级固件。

## 配置与上传验收

在助手中为 Kindle 单独扫码，建立独立 `deviceId`；逐项确认 `local_book_id / weread_book_id / title` 映射。旧网页版中明确确认的对应关系可以带入，旧阅读时长和未决提交不能迁入设备空账本重试。

纯新用户先交接并导入**空映射**配置，使 worker 能解析交接后的新增阅读；随后正常新读、关书并在 KUAL 导出状态，接回助手点“读取已解析书籍”取得精确本地 ID。只读搜索微信读书的具体版本，编辑并“保存书籍对应”，再导出、导入更新配置。已有旧映射的用户可点“载入旧书籍对应”核对后复用。映射更新仍会使旧验收绑定失效，不会直接放开上传。

交接与导出默认保留两个验收开关为 `false`。安全弹出后，在 KUAL 选择 `Import USB configuration`，凭据会被导入设备私有目录并清理已验证的 USB 暂存文件。**导入配置不会自动开启上传。**

只有真实设备、性能和协议验收完成后，才允许维护者整理与当前配置绑定的验收证据。小样试传需一个新增真实完整会话、明确计划及人工确认；不能将长会话剪成虚构短样本。验收矩阵见 [真机验收清单](docs/DEVICE_VALIDATION.md)，门禁格式见 [worker 说明](device/worker/README.md)。

服务端成功回执不足以确认到账：必须同时核对单书增量、目标日期增量及原有阅读位置。结果未知时封锁重放；仅明确成功回执但统计不符的记录可做只读复核。本项目不能承诺微信读书永久免扫码或接口长期兼容。已有凭据时，助手拒绝直接覆盖身份；续期失败后的重新授权需要先核对并保留设备账本，不能通过删除凭据、重扫新身份来跳过未决记录，见 [配置维护说明](docs/CONFIGURATION.md#重新授权与身份维护)。

## 空间、维护与隐私

PW2 配置限制原始队列 **4 MiB**、SQLite 账本 **4 MiB**，为例行事务保留 **4 MiB + 64 KiB**，同时尽力保留至少 **20 MiB** 系统空间；安装还计入元数据、完整新组件及旧版恢复副本。容量不足会阻断，不删除未确认记录或亚马逊数据。系统其他进程也会使用此分区，空间检查不是独占预留。

设备凭据、账本和运行组件位于 `/var/local/kindle-native-sync/`；Mac 本地资料位于 `.weread/`。USB 的 `.kindle-native-sync-inbox/` 只是临时凭据交接区，FAT 不具备 Unix 文件权限保护，导出后应及时在设备导入。诊断默认仅导出枚举状态、大小与资源计数，不包含原始日志、电子书、令牌或完整序列号。

KUAL 提供启用、停用、状态导出与卸载。卸载保留账本与凭据，避免重装后重复上报；不要通过删账本来重试未知请求。只清理能证明已经确认的源记录，默认不执行 SQLite `VACUUM`。升级会撤下旧设备验收证据；断电残留需人工复核，不能盲目重复安装。

## 离线开发测试

```sh
python3 -m unittest discover -v
python3 -m unittest device.install.test_transactions
make -C device/collector test
cargo +1.91.0 test --locked --manifest-path device/worker/Cargo.toml
cargo +1.91.0 fmt --check --manifest-path device/worker/Cargo.toml
```

测试使用合成日志、临时目录和模拟传输，不使用真实账号或真实上传。日志轮换、断网、突然退出、认证过期、未知回执及磁盘不足均须保留防重复边界。软件测试通过不能代替翻页延迟、耗电或真实入账验收。

项目整体采用 **AGPL-3.0-only**，第三方文件保留原许可；见 [LICENSE](LICENSE)、[第三方声明](THIRD_PARTY_NOTICES.md) 和 [贡献指南](CONTRIBUTING.md)。这是独立社区项目，与 Amazon、腾讯及微信读书无关联。
