# 一次性预览维护检查

`Check preview / restore read-only / export private diagnostic` 供本次未配置的 `5423150` 候选检查使用。它先核验 PW2/B0D4、5.12.2.1.1、root、原候选的固定 SHA256SUMS 摘要和全包校验。设备私有目录只要已有 `config.json` 或 `auth.json` 就拒绝执行；其他候选也会拒绝，不能拿这个入口作为通用升级器。

它不重装程序，不停止或启动服务，不读写账号、账本、配置或验收记录，不删除未决事务标记。仅调用设备自带 `mntroot ro`，并从所有 `/proc/mounts` 的 `/` 项核验真实挂载状态；`rootfs` 伪条目的 `rw` 不单独判定为实际系统分区可写。非零原生命令结果仍会保留，成功与否以实际挂载后置条件为准。

此入口是**用户显式请求的私有诊断导出**，不同于常规脱敏状态：它会复制最多 4 MiB 的本项目已封存事件日志，其中有书名/阅读时间/位置及电源事件。不会复制活动日志、整个 `/var/local`、系统网络/WPA日志、电子书或凭据。不要把导出目录提交到 GitHub。

输出位于 USB 的 `.kindle-native-sync-export/maintenance-时间-PID/`；根目录的 `maintenance-result.json` 指向本次结果，旧 `install-result.json` 保持不变。诊断包含所有根挂载 source/type/options、原 `mntroot` 的路径、hash及不超过64KiB的脚本副本、本项目组件hash、事务标记存在状态、`initctl status` 和逐步骤退出码。日志按固定清单和长度复制，复制前后及副本的SHA必须一致。

安全弹出、拔线后从 KUAL 点击一次，等待操作结束再接回 Mac。输出失败或信号中断仍会尝试不依赖 USB 输出的原生 `mntroot ro` 收尾。报告为 `needs_review` 时保留全部证据，不重复安装或删除账本。它不把旧安装失败回执改为成功，也不代表休眠、性能或微信读书入账通过验收。
