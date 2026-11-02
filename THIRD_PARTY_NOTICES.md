# 第三方来源和许可

Copyright (c) 2026 WanhaoZhang。项目新增代码整体采用 **AGPL-3.0-only**；本声明不更改第三方文件原有版权与许可。完整本项目许可见 [LICENSE](LICENSE)。

本项目设备代码拆分自 [kindle-native-reading-sync 的 `54231504fcfda32c3c21ecc1f143119e79982ea8`](https://github.com/WanhaoZhang/kindle-native-reading-sync/tree/54231504fcfda32c3c21ecc1f143119e79982ea8)，源码关系见 [拆分说明](docs/REPOSITORY_SPLIT.md)。

| 项目 | 固定来源提交 | 使用范围 | 原许可及保留材料 |
| --- | --- | --- | --- |
| [huangziwei/readinglog](https://github.com/huangziwei/readinglog) | `3d5d5b394818387c42a35834e743d1b13aec6b60` | `vendor/readinglog/` 的 workspace/native/desktop 源码，供 Rust worker 调用原生日志、计时和副文件解析能力；保留上游 workspace 上下文 | GPL-3.0-or-later；见 [vendor/readinglog/LICENSE](vendor/readinglog/LICENSE) 及各源文件声明 |
| [miumiupy98-art/miuread-koreader](https://github.com/miumiupy98-art/miuread-koreader) | `ee9d6f24768349d428230036087c379dd957b30c` | 阅读时间 DTO、小时分配、签名与协议行为参考，移植见 `device/worker/src/protocol.rs` 及网络/账本实现；未包含其 Lua UI | AGPL-3.0-only；[项目 LICENSE](LICENSE)、[MiuRead NOTICE](licenses/MiuRead-NOTICE.txt)、[MiuRead 第三方声明](licenses/MiuRead-THIRD_PARTY_NOTICES.txt) |
| [AnkioTomas/moon](https://github.com/AnkioTomas/moon) | `43b2181da6eb5285b62114da85e1413d2f8464e4` | 上述协议签名变换的原始来源 `book.koplugin/source/wechat/app.lua`，Rust 移植位于 `device/worker/src/protocol.rs` | GPL-3.0；见 [licenses/GPL-3.0.txt](licenses/GPL-3.0.txt) |
| [teng-lin/weread-omni](https://github.com/teng-lin/weread-omni) | `88bd2e095d7d7ee423eaadf8f40653e72c5be6d4` | `weread_readonly.py` 的 QR 登录、E-Ink 请求参数和协议参考；Copyright (c) 2026 Teng Lin | MIT；见 [licenses/weread-omni-MIT.txt](licenses/weread-omni-MIT.txt) |

本仓库不分发原网页版的 `weread_upload.py`、KRDS Python 解析器或 Kindle Reading Dashboard 代码，因此不以那些文件作为本仓库的运行依赖。保留的 ReadingLog 桌面/UI 源文件是上游 Rust workspace 与来源上下文的一部分，不意味着本项目构建或发布该 UI。

二维码显示使用 `qrcode` / Pillow；Rust 直接与间接依赖以 `device/worker/Cargo.lock` 为准，各依赖保留各自许可。构建需要的工具链由开发者自行获取，未把 NiLuJe 或 Zig 工具链二进制纳入本仓库。

协议移植增加了独立设备身份、来源与时钟核验、持久账本、发送前登记及回查门禁；不等同于上游客户端逐字段行为，也不代表微信读书官方支持外部阅读时长导入。测试中的签名向量和 guest 常量来自公开协议实现，不是用户的私人 access token。
