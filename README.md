# Codey

Codey 是 Codex 桌面客户端的增强启动器。打开 Codey 后，它会自动拉起 Codex，并在 Codex 页面内提供统一的 Codey 控制台，用来管理线路、模型、会话、通知、插件、更新和运行策略。

## 功能描述

- 自动启动 Codex，并在页面顶部提供 Codey 控制台入口；可查看运行状态、重启由 Codey 启动的 Codex，并检查、下载和安装 Codey 更新。
- 管理官方登录线路、第三方线路和 CC Switch 线路；支持保留官方账号登录、沿用路由代理、热切换线路，并兼容手工配置的 Codex 中转线路。
- 在官方账号线路下显示可拖动的额度浮窗，展示套餐、5 小时和 7 天额度的剩余比例与刷新时间，也可在控制台关闭。
- 同步当前线路的可用模型，设置默认模型，并管理第三方线路的自定义模型；首次使用时也可直接保存手动输入的模型，无需先切换官方线路启动 Codex。保存后会尽量立即刷新 Codex 的模型选择器，失败时按界面提示重启生效。
- 增强会话管理：显示更友好的会话时间，支持导出、导入、删除指定轮次、恢复最近备份，并提升历史会话恢复到原项目的稳定性。
- 增强插件市场与页面体验：修复官方和本地插件展示，支持一键检查并尝试修复插件市场，保存个性化页面增强设置，屏蔽部分干扰提示，并可跟随或固定官方试验性功能开关。
- 提供启动与平台体验优化：减少 Codex 卡在启动页的情况；Windows 可选择 Codex 安装目录、显示首次启动失败提示，并提供渲染启动策略用于排查显示异常。
- 支持按需精简宠物和语音功能，关闭后可恢复完整体验。
- 支持上下文工具增强，改善长任务中的文件读取、搜索和替换体验；内置工具来自 [yc-duan/fastctx](https://github.com/yc-duan/fastctx)，已经配置自己的 FastCtx 时会优先沿用。
- 支持子代理协作优化；开启后可单独指定子代理模型和思考深度，切换线路时会自动回到可用默认值，运行中保存的调整会用于后续新启动的子代理。
- 提供诊断日志统计、清理和写入保护，帮助控制本地磁盘占用。
- 支持多个飞书机器人（包括企业内网部署）和 Telegram 通知渠道，可单独启停、删除和测试，并在任务完成、失败或等待介入时发送提醒。

## 使用方式

打开 Codey 后，它会自动启动 Codex。进入 Codex 后，点击顶部的 “Codey” 按钮即可打开控制台。模型变更通常会立即更新；其他需要重启才会生效的开关或线路变更，请按界面提示保存并重启 Codex。

## 注意事项

- Codey 面向 Codex 桌面客户端，不覆盖命令行版本。
- 第三方线路是否可用取决于对应服务本身的能力与账号配置。
- 保留官方账号登录只保留 Codex 的账号状态，不会把已选择的第三方线路切回官方接口。
- 删除、导入和恢复类会话操作会尽量保留备份，但仍建议谨慎使用。
- Windows 和 macOS 上启动 Codey 时，如果 Codex 已在运行，Codey 会先将其关闭再重新启动；正在运行的任务会被中断，请提前保存重要内容。
- 部分增强能力依赖当前 Codex 版本和当前线路支持情况，遇到不兼容时请以 Codey 控制台提示为准。
- mac arm版本因无签名原因会报损坏，运行`xattr -dr com.apple.quarantine /Applications/Codey.app`即可跳过

## 第三方声明

    This product includes FastCtx
    (https://github.com/yc-duan/fastctx), Copyright (c) 2026 yc-duan,
    used under the Apache License 2.0.

    FastCtx is redistributed and/or modified here by the maintainer of
    this distribution. Any such change is that maintainer's own work
    and their sole responsibility. It is not endorsed by, not
    supported by, and not attributable to the author of FastCtx, who
    accepts no liability of any kind arising from this distribution or
    from anything built on top of it.

## 联系方式

Codey 由 [SuperGness](https://github.com/SuperGness) 创建和维护。集成、再分发、合作或任何其他事宜，欢迎联系：kimzane9991@gmail.com。

## 致谢

感谢 [linuxdo](https://linux.do/) 社区的讨论、分享与反馈。
