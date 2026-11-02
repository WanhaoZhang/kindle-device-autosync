# PW2 可逆 OTA 防护

这是 **PW2 / B0D4 / 5.12.2.1.1** 的受控维护入口，不升级固件、不打开 Wi-Fi、不结束系统进程。它把 `/usr/bin/otaupd` 与 `/usr/bin/otav3` 原地重命名为同目录 `.bck`，保留字节与文件权限；Restore 按已保存哈希反向恢复。

方案参考 hius07 的 `renameotabin`：[KindleModding Legacy 指南](https://kindlemodding.org/jailbreaking/Legacy/post-jailbreak/disable-ota.html)、[原作者发布帖](https://www.mobileread.com/forums/showthread.php?t=327879&page=2)、[PW2 / 5.12.2.1.1 的一手使用报告](https://www.mobileread.com/forums/showthread.php?t=327879&page=5)。指南要求 FW >=5.11 使用二进制重命名，并在重启后才重新启用 Wi-Fi；旧 `update.bin.tmp.partial` 文件夹不作为本固件的防护证明。

本项目独立实现预检、哈希、状态记录及受控失败回滚，没有打包原作者的无检查 shell 脚本。2026-11-02 核验的[指南原包](https://kindlemodding.org/jailbreaking/Legacy/post-jailbreak/renameotabin.zip)为 1,031 字节，SHA-256 为 `d2d333620420096e040e2f195c05bebfc97bdeeb5adf26b04c21a254e48ac439`；该值只记录参考来源，不是本项目脚本的签名。

## 接口与联网顺序

在 Kindle root / KUAL 环境，从完整候选包调用：

```sh
sh /mnt/us/kindle-native-sync-package/ota/control.sh status
sh /mnt/us/kindle-native-sync-package/ota/control.sh guard
# 保持飞行模式，按 KUAL 的明确重启步骤完成一次重启。
sh /mnt/us/kindle-native-sync-package/ota/control.sh require-ready
```

`guard` **不自行重启**。部署菜单可在其它准备动作完成后统一重启；也可由用户主动重启。重命名时旧 OTA 进程可能仍在当前内存中，因此仅看到 `.bck` 文件不够。脚本记录 `/proc/sys/kernel/random/boot_id`；重启之前 `reboot_required=true`、`ready_after_reboot=false`。只有启动 ID 已改变、两份 `.bck` 哈希匹配、原路径不存在、真实根挂载为只读、USB 用户区可读取且没有待装固件、没有中断标记时，`require-ready` 才返回 0。

`status` 是只读查询，返回 JSON；它的退出 0 只表示报告已生成，**不表示可以联网**。自动化入口必须使用 `require-ready` 的退出码和 `ready_after_reboot`。未知、未防护、未重启、哈希不符或部分重命名均返回非零。

## 预检与恢复

- 要求设备型号、固件、root 与私有目录检查通过；所有原件须为普通可执行文件且不是符号链接。
- Guard / Restore 必须保持飞行模式；脚本只接受 Wi-Fi 的 `NA`、`DISABLED` 或 `UNAVAILABLE` 状态，连接中、已连接及未知状态均拒绝，不主动改变网络。重启后的 `require-ready` 只读检查可以在联网时执行。
- 已有 `.bck`、不匹配的私有记录或未解决的事务标记会被拒绝，不覆盖来历不明的备份。
- USB 根目录的真实 `*.bin`、`*.bin.*`、`update*.tmp*`、`update*.partial*` 文件或目录会阻断操作。AppleDouble `._*` 元数据忽略。脚本**不会删除**这些文件来继续安装；需先人工核对并移出真正的待装固件。
- `/var/local` 必须仍保留至少 20 MiB 系统空间，加 512 KiB 小型维护记录余量。
- 私有记录位于 `/var/local/kindle-native-sync/ota/`，仅包含操作、启动 ID 和二进制哈希，不含账号。每次重命名前持久登记意图；两个名称各自原子重命名，发生受控命令失败或信号时尝试按原哈希反向恢复，核验真实根只读后才清事务标记。
- 这不承诺两个文件在突然断电时整体原子回滚。中断标记保留并封锁后续操作，需维护者核对原件、备份及 `intent.tsv` 后恢复；不能盲目删除标记或重复点击。

恢复 OTA：

```sh
sh /mnt/us/kindle-native-sync-package/ota/control.sh restore
# 按明确的用户/KUAL步骤重启。
```

Restore 只接受本脚本管理且内容完全匹配的两份备份。它不会接受其它扩展创建的无来源 `.bck`。恢复后防护不再生效，应在仍处于飞行模式时完成计划中的系统维护。重置、手动升级或恢复 OTA 原件前，先确认没有未决阅读上传。

## 离线回归

```sh
python3 -m unittest test_ota_guard
```

测试使用临时文件系统、假挂载命令与合成二进制，不读取真实设备或账号。覆盖待装固件、备份冲突、符号链接、哈希变化、第二次重命名失败、只读恢复失败、信号、空间不足、启动 ID 和中断标记。
