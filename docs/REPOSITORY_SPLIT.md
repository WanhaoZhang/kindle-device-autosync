# 仓库拆分与来源

2026-11-02 将原混合仓库划分为两个独立项目：

| 仓库 | 职责 |
| --- | --- |
| [kindle-native-reading-sync](https://github.com/WanhaoZhang/kindle-native-reading-sync) | Mac 网页导入、每日统计、人工核对与时长同步 |
| [kindle-device-autosync](https://github.com/WanhaoZhang/kindle-device-autosync) | Kindle C 采集器、Rust worker、KUAL/Upstart 包及首次 Mac 配置助手 |

设备端的拆分基线是原仓库提交 [`54231504fcfda32c3c21ecc1f143119e79982ea8`](https://github.com/WanhaoZhang/kindle-native-reading-sync/tree/54231504fcfda32c3c21ecc1f143119e79982ea8)。原历史保留在原仓库，新仓库保留相应版权及第三方固定来源。拆分不是重新授予第三方代码许可，也不更改已生成候选包的来源标识。

## 本仓库包含

- `device/`：采集器、事件适配器、Rust 解析/同步 worker、KUAL、安装/升级/卸载与构建工具。
- `device_app.py`、`config-web/`、`device_bridge.py`：独立 Mac 配置和诊断入口；不带 Mac 阅读时长提交入口。
- `weread_readonly.py`：专用扫码及受限只读协议辅助。
- `vendor/readinglog/`：worker 所需的固定上游 Rust 解析源码和 Cargo workspace 上下文。保留的 desktop/native UI 源文件用于完整的上游 workspace/来源上下文，不是本项目另一个发行应用；设备 worker 不频繁启动 ReadingLog UI。
- `licenses/`、`LICENSE`、`THIRD_PARTY_NOTICES.md`：本项目及第三方来源与许可。

不带原网页版的 `local_app.py`、阅读导入页面、Python 上传器、报告生成和 Mac 日志解析适配器。设备构建、配置和离线测试不要求再克隆网页版。

## 运行数据不随仓库迁移

凭据、二维码、日志、副文件、电子书、数据库和构建产物均不作为拆分输入，也不发布到 GitHub。仅从原仓库受版本管理的源文件挑选代码。

已有用户的真实工作目录继续保留。新设备身份不会重新上传旧账本的未决记录；迁移时，必须通过配置助手指定实际旧目录完成上传权交接。旧待核验数据留在原实例，新设备只从交接后的新窗口记录。纯新用户可显式建立新绑定，不需要安装旧项目。

原来已经暂存或生成的 `5423150` 预览包没有因为仓库拆分被自动重写、替换或安装。其 manifest、SHA256 与对应源码仍指向原提交；今后在本仓库新构建的候选包应指向本仓库准确提交及源码 hash。一个包被复制到设备不代表常驻服务已安装或验收通过。

## 当前验收边界

本台 PW2 5.12.2.1.1 已完成 root 探针与无网络 ARM worker 自检。常驻采集、睡眠/USB循环、联网 TLS、性能、真实独立入账及一周观察仍需完成。拆分和公开源码不改变这些状态，自动写入仍默认关闭。
