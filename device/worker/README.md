# ksync-worker（设备端单次进程）

此 crate 只实现设备端解析、账本和同步 worker。没有 UI，不接管 Kindle 阅读器，不打开 Wi-Fi，不设置 RTC 唤醒或阻止休眠。默认 `sync` 为 collect-only；只有正式验收，或下述明确授权且真实短试传到账的实验链路，才允许自动上报。编译通过不代表验收通过。

## 编译与测试

```sh
cargo +1.91.0 test --locked --manifest-path device/worker/Cargo.toml
cargo +1.91.0 build --locked --release --manifest-path device/worker/Cargo.toml
```

在 Mac 上编出的程序只是本机测试版。ARM 目标是 `armv7-unknown-linux-musleabihf` 的完全静态 ELF；bundled SQLite 还需要目标一致的 musl C 编译器或受控 cross 工具链，不能混入 softfp/glibc 对象。二进制能编译不代表 PW2 旧内核、TLS、功耗已验证。

## 根目录和调用

私有 root 默认 `/var/local/kindle-native-sync`，目录 `0700`；认证、配置、数据库为 `0600`。仅读取 collector 已 fsync/封存的 `events/sealed/*.log`，不打开活动 `active.log`。

```sh
ksync-worker --root DIR self-test
ksync-worker --root DIR self-test --online-self-test
ksync-worker --root DIR import-config --inbox /mnt/us/.kindle-native-sync-inbox
ksync-worker --root DIR collect
ksync-worker --root DIR sync
ksync-worker --root DIR status
ksync-worker --root DIR export --out /mnt/us/kindle-native-sync-export
ksync-worker --root DIR compact
ksync-worker --root DIR reconcile --plan-id PLAN_ID
```

`self-test` 默认仅探测实际架构、B0D4 硬件前缀（对 `/proc/usid` 严格只读取 4 字节，不读取完整序列号）、固件、内核和私有原子文件读写，**不会联网或生成验收证书**。显式 `--online-self-test` 才向固定的 `https://i.weread.qq.com/config` 匿名 GET 一次，保留标准 CA/主机名校验，不读取或发送账号凭据。

`status.books` 仅列出已解析会话中的精确 `local_book_id` 和书名，包含尚未匹配的书，用于独立配置助手首次建立对应；不导出源日志、计时来源行或认证资料。新安装可先导入空映射，再新增阅读并导出状态，取得此清单后保存准确目标映射。该只读元数据入口不放宽采集、真实入账或设备验收门禁。

`collect` 需要 runtime 新鲜（120 秒以内）、reader idle 20 秒、设备 awake、非 USB 导出、capture 未阻断；不要求联网。`sync` 额外要求联网且不存在待人工核验的 capture gap。所有未知状态、缺字段、未来时间均阻断。重启后的 bootstrap 与 Mac handoff cutoff 取较晚值，避免导入首次安装以前的数据。未配置时 raw journal 仍由 collector 保留，worker 返回 `collect_only_unconfigured`，不会尝试账号请求。

计时解析从该 cutoff 之后第一个真实 `ReadingTimerController:Information::OpenBook` 及其原生 `StoredBookData` 基线开始。PW2 的 `ReaderBooklet` 关书日志可能不会让上游分割 sitting；若先解析全部旧日志再丢弃跨 cutoff 的 sitting，会把后来重新开书的合法新阅读一起丢掉。本项目在解析前选择新的完整开书窗口，不按时间裁剪秒数、不制造 `CloseBook`。原始来源、指纹、精确身份及 UTC 锚仍完整保留，原有时钟、小时守恒、去重与重叠检查继续执行。cutoff 之后只有翻页却缺少新开书基线时，返回 `awaiting_post_cutoff_open=true`，保留来源并阻断清理，不能把旧会话尾部当作新阅读补传。

手动 trial 的 prepare/confirm 与 `reconcile` 若发现 runtime 已超过 120 秒，会先向私有 `adapter-control.fifo` 非阻塞请求 `refresh`，核验 FIFO 类型、属主、0600 权限和打开前后 inode/device 一致，再等待 collector 真正更新 runtime；不 touch 文件或改写状态。总等待最多 6 秒，真实快照过程中短暂不安全状态最多等待 2 秒，最后仍必须通过同一 gate。reader active/unknown、无 adapter reader、符号链接或宽松权限均继续阻断。自动 sync 在同次自然唤醒内按书籍及日期逐笔排空；每笔前仍检查取消、真实空闲和网络状态，runtime 过期才请求同一个真实快照刷新，不新增周期唤醒。任一不明写入停止整轮，后续不会重发。

## 配置、认证和交接

`config.json` schema 1：`device_id`, `account_vid`, `profile`（`pw2-5.12.2.1.1`）, `firmware`（`5.12.2.1.1`）, `timezone`（`Asia/Shanghai`）, `bootstrap_cutoff_unix`, `mappings`（每项 `local_book_id/weread_book_id/title`）, `validation`。

`auth.json`：`vid/deviceId/accessToken/refreshToken`。设备需要独立的 deviceId 和授权；不得复制 Mac 现有并行使用的上传身份，也不得把旧的 455 秒待核验数据导入新账本重放。

`import-config --inbox DIR` 从 `config.json/auth.json/handoff.json` 导入，manifest 为：

```json
{"schema_version":1,"kind":"kindle-native-sync-handoff","config_sha256":"SHA256_OF_RAW_CONFIG_BYTES","auth_sha256":"SHA256_OF_RAW_AUTH_BYTES"}
```

worker 检查文件 hash、账号和设备一致，原子保存到私有 root。交接配置中的两个验收开关必须都是 false。成功后仅删除已验证的三份交接文件，若 inbox 为空才删除目录，不删除额外用户文件。导入失败不清除凭据供调查；应在 USB 用户区上及时人工清理失败交接的凭据。身份、账号或 bootstrap 不能覆盖已有账本。

`status.capture` 只提供已枚举的 runtime 状态和 `active_bytes/sealed_bytes/sealed_files` 数字，便于尚未配置账号时验证采集链路，不读取日志正文或认证数据。runtime 缺失、权限/类型不安全或状态枚举非法时 `available=false`；该快照不是设备验收证据。

## 两项验收和受控试传

`validation` 为：

```json
{"device_validation":false,"protocol_validation":false,"binding_sha256":""}
```

绑定 hash 为配置**去掉顶层 validation**后的 JSON：UTF‑8、键递归排序、无额外空格，与 Python `json.dumps(obj,sort_keys=True,separators=(',',':'),ensure_ascii=False)` 相同。`status` 输出 hash 便于人工核对，但不输出 vid 或认证令牌。

正式启用前必须先在实际 PW2 上验证 ELF、睡眠/唤醒、USB 循环和性能，将真实记录整理为 `ROOT/validation-device.json`，模板见 `examples/validation-device.example.json`。模板默认全部 false，不会自动导入或转成 true；`kernel_release` 及配置 hash 必须与现场一致。真实写入另检查实际 ARM、root、B0D4 前缀和系统固件文件，不能在 Mac 修改配置布尔就上传。

通过正式设备验收，或下述真实设备和 TLS 自检及显式实验授权后，可对**安装后新增的真实阅读**进行人工试传：

```sh
ksync-worker --root DIR sync --trial --prepare --book-id LOCAL_KEY --max-seconds 60
# 人工核对输出中的完整会话、来源、目标书、日期和秒数，保存精确 plan_id
ksync-worker --root DIR sync --trial --confirm --plan-id PLAN_ID
```

trial 优先选择所选书最新的完整窗口，必须是一个可核验的 counted 会话，1–60 秒；不会把长会话剪成看似真实的小片段。正式验收路径需已绑定的 `device_validation=true`，允许 `protocol_validation=false` 研究首次协议验收；显式实验路径则需真实设备/TLS证据及用户独立授权，不伪造验收布尔。准备阶段不联网，确认必须指向同一不可变 plan_id。

正式验收路径的自动 `sync` 必须两项验收全部通过且 binding 相同；显式实验路径见下一节，仍须真实试传精确到账。协议验收应覆盖同书连续会话、跨小时、历史日期、阅读位置保护和不明写入不重放，不能只凭一次 HTTP 200 或 succ=1 打开开关。合并关书会话的行为尚不能宣称已解决服务端“成功但未记账”。

## 显式实验性自动使用

长期性能、深度休眠和完整服务端多场景矩阵未完成时，维护者可以在用户明确接受这些未验证项后启用**实验性**路径，不把 `device_validation` 或 `protocol_validation` 伪装为 true。

1. 导入配置后运行 `self-test --online-self-test`。只有真实 ARM/PW2/root/固件，以及匿名 HTTPS 的标准 CA/主机名校验成功，才保存 `validation-tls.json`，绑定配置、实际内核和当前 worker 的 SHA256。失败时 CLI 仍打印诊断 JSON，但退出非零。
2. 显式 `experimental-authorize-trial --accept-unverified-performance` 建立只允许人工短试传的授权。没有账号匹配或真实 TLS 证据时拒绝。
3. 准备、人工核对并确认一个新增完整会话（1–60秒）。仍执行同样的来源、时钟、小时、映射、幂等账本、一次 POST 和三项回读检查。
4. 只有该授权及**当前 worker**下的试传在同一私有账本中实际成为 `confirmed`，才可 `experimental-enable --plan-id ID --accept-unverified-protocol`。仅 `succ=1`、`needs_review`、未知请求或旧 worker 的回执不能启用。
5. `experimental-disable` 仅撤下实验性上传授权，保留全部队列和回执。配置、映射、内核或 worker 变化会使旧授权失效。

无 SSH 的 KUAL 操作使用 `operate --inbox DIR --action ACTION`，只读取 `DIR/request.json`（最多16KiB、常规文件、拒绝符号链接）。请求 schema 1，必须含 `action` 与精确 `config_binding_sha256`；可用字段是 `local_book_id`、`max_seconds`、`plan_id`、`accept_unverified_performance`、`accept_unverified_protocol`，拒绝其他字段。

动作包括 `authorize-trial`、`prepare`、`confirm`、`enable-experimental`、`disable-experimental`。组合动作 `authorize-prepare` 在真实 TLS 后授权并解析/准备；`confirm-enable` 必须在发送前明确接受未完成协议矩阵，只在确认同一计划实际到账后启用，随后在同一进程和账本锁中排空其余合格积压，结果列在 `automatic_drain`。未知或待核验结果不会启用，不重放原 POST。

已授权的自动 `sync` 会按书籍、日期逐笔排空可验证积压；每笔独立持久登记并回读。新开书、睡眠、断网或取消会在下一笔前停止；已经发出的唯一当前请求只做有限收尾。任何未知/待核验提交停止整个排空，不会通过轮询重发。程序不主动开 Wi-Fi、不阻止休眠、不增加定时唤醒。

## 解析和不可重放边界

复用 ReadingLog 的解析、counted 模型和小时分配；BufferedLogger 延迟内容只补精确身份，不增加秒数。副文件统计索引按文件 inode/尺寸/mtime 缓存。cc.db 以只读方式打开并缓存，仅为已精确识别的 sidecar/内容键补标题；不使用文件字节大小猜测 HTMLPosition，不改变已识别的 source key。旧日志仍要求同日三条去重后的 JournalSaver UTC+08 锚。新采集器的 `KSYNC_CLOCK_V1 native_wall=... observed_unix=...` 时钟见证必须与按 +08:00 换算的原生日志时间相差不超过 5 秒；同一完整阅读窗口至少还需一条真实 JournalSaver UTC 锚，不能仅凭采集器自证。跨午夜窗口必须同时具有自身开书/关书两端见证及自身关书 UTC 锚，才能核验两日小时分配；无证据的数据继续保留并阻断。

每个 plan 在网络写入前持久 claim 为 `dispatching`；之后仅发送一个 POST，再最多两个 GET 回读。认证只在写前 GET 预检阶段续期一次，核对同 vid 并原子保存。POST 后不续期、不重发。仅在单书累计量、正确日期总量和位置均满足预期时确认。`reconcile` 仅对明确收到成功回执的 needs_review 批次做 GET 核验，不续期、不 POST。

SIGTERM/SIGINT 只设置取消标记：发送前退出，已 claim 进入发送边界则允许当前一个 POST 与两个有限 GET 收尾，再退出。单请求 HTTP budget 20 秒，此阶段最多三次请求，不阻止系统睡眠；真实断电/强制 kill 留下 dispatching 仍阻断重放。

counter 范围重叠默认阻断；OpenBook 的整秒 TimeRead 可能造成不足 1 秒的边界重叠，只有字数边界精确相邻、旧结束早于新开始且有新的明确 OpenBook 时才按上游已有分配保留为新会话。不自行重新分配小时或补造缺失时间。

## 空间、内存与 compact

PW2 统一存储 profile：SQLite `page_size=4096,max_page_count=1024`，账本硬上限 **4 MiB**。`Store::open`/schema 初始化、每个 collect/prepare/claim/receipt/reconcile/compact 的写事务前，均重新检查真实可用块：至少保留 **20 MiB 系统 free + 4 MiB 例行事务/rollback + 64 KiB 元数据与分配对齐余量**。JSON 原子替换另外按新临时文件实际字节预算，再保留相同系统与元数据余量。已有超过 4 MiB 的账本会被保留并拒绝修改，不截断或自动重建。

保持 `journal_mode=DELETE,synchronous=FULL`，关闭 cache spilling，避免大型事务中间溢写造成额外 journal 分段；SQLite 临时排序放内存，受账本和输入窗口上限约束。若 POST 之后系统空间不足以安全写回确认，保留已 claim 的 dispatching 账本，不通过重发来“修复”。Collector 应使用同一 4 MiB raw 队列与 24 MiB+64 KiB free 阈值，并在 append 前读取实时 free，不能沿用过时的 worker 写入前 free 缓存。

一次解析的封存 raw journal 和 raw line ledger 上限为 4 MiB，超过返回 `memory_budget_blocked` 并保留源文件。SQLite 逐行分类，不构建多份完整正文；**worker 真机峰值 RSS/翻页延迟仍未实测**，4 MiB 输入限制不是内存占用保证。

`compact` 仅在最近一次解析没有任何无法关联/时区不明/冲突数据、所有 sessions 与 attempts 都 confirmed、来源 hash 均可核验时执行。事务清理 raw lines/sources，归档源 hash，保留会话 ID、counter 范围、日统计和上传回执，再仅删除这些已确认的 sealed 日志。**默认不执行 VACUUM，也不缩小 SQLite 文件**；腾出的页面会在 4 MiB 文件内复用。VACUUM 可能另需约两份当前数据库的临时空间，不能用例行 4 MiB 预算覆盖；本版不提供未暂停 collector 的 maintenance vacuum。活动日志不碰；任何未确认或无法解释的片段都会阻断清理。自动 sync 成功后也会尝试该保守清理，受新一轮 runtime/取消检查约束。

无法安全 compact 时应导出、人工诊断，不能删除 ledger 重新开始。即使 compact 后原始日志的重复复制件再次出现，保留的 ID 和 counter 范围也能阻断重复上传。

设备计时只解析真实 `OpenBook` 到原生 `CloseBook` 或 `ReaderBooklet stop`/`End of stop method` 的完整窗口。同一书后续重新打开会独立形成计数窗口；不会把已确认的旧段和后读段合成重叠区间，也不会裁出未关闭的尾部。秒级日志中 stop 与 End of stop 同秒时，以真实同秒 stop 证据配对，防止日志去重排序改变其先后造成漏采。
