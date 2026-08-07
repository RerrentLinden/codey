# Codey

Codey 是 Codex 桌面客户端的增强启动器。打开 Codey 后，它会自动拉起 Codex，并在 Codex 页面内提供统一的 Codey 控制台，用来管理线路、模型、会话、通知、插件、更新和运行策略。

## 功能描述

- 自动启动 Codex，并在页面顶部提供 Codey 控制台入口；可查看运行状态、Codex 版本和应用路径，重启由 Codey 启动的 Codex，并检查、下载和安装 Codey 更新。启动时页面会提示正在检查更新，最多等待 10 秒；更新源不可用时会自动跳过，发现可安装的新版本时会先询问是否更新。
- 管理官方登录线路与第三方线路，线路信息始终以 Codex 当前配置为准；兼容手工配置及 CC Switch 已写入 Codex 的当前线路。CC Switch 中标记为需要路由的兼容线路也可关闭其路由，由 Codey 在启动时自动适配；继续启用 CC Switch 路由时，真正的线路变化会触发一次安全重启，客户端仅整理配置格式不会造成反复重启，避免新线路误用旧地址或旧凭据。
- 在官方账号线路下显示可拖动的额度浮窗，展示套餐、5 小时和 7 天额度的剩余比例与刷新时间，也可在控制台关闭。
- 同步当前线路的可用模型，设置默认模型，并管理第三方线路的自定义模型；模型较多时可搜索并分批浏览。CC Switch 路由开启时会从其当前源 API 获取模型列表，不会把路由占位凭据发送给源服务。首次使用时也可直接保存手动输入的模型，无需先切换官方线路启动 Codex。可通过“测试对话”提前检查当前第三方线路、凭据和默认模型是否可用；兼容线路会在启动时自动适配 Codex 的对话格式。保存后会尽量立即刷新 Codex 的模型选择器，失败时按界面提示重启生效。
- 增强会话管理：显示更友好的会话时间，支持导出、导入、删除指定轮次、恢复最近备份；启动时会先确认当前线路完整有效，线路切换尚未完成时驻留等待恢复而不会反复退出，再按稳定线路修复历史会话归属，并提升历史会话恢复到原项目的稳定性。
- 增强插件市场与页面体验：修复官方和本地插件展示，支持一键检查并尝试修复插件市场，保存个性化页面增强设置，并屏蔽部分干扰提示。
- 提供启动与平台体验优化：减少 Codex 卡在启动页的情况；Windows 会限制异常密集的后台 Git 状态与审查请求，降低大量短时 Git 进程造成系统资源耗尽的风险；还可选择 Codex 安装目录、显示首次启动失败提示，并提供渲染启动策略用于排查显示异常。
- 支持按需精简宠物功能：默认收起宠物、隐藏宠物入口，并避免在启动时预先创建隐藏的宠物窗口；主动使用官方语音功能时仍可按需启用共用能力，关闭精简后可恢复完整宠物体验。
- 支持上下文工具增强，改善长任务中的文件读取、搜索和替换体验；内置工具来自 [yc-duan/fastctx](https://github.com/yc-duan/fastctx)，已经配置自己的 FastCtx 时会优先沿用。
- 支持子代理协作优化；开启后可单独指定子代理模型和思考深度，当前线路中未被明确禁用的可用模型都可作为子代理，不再固定为少数模型，旧版 Codex 客户端也会自动适配。切换线路或刷新线路模型后会自动回到可用默认值；当前线路没有兼容模型时会关闭优化，避免后续子代理启动失败。运行中保存的调整会用于后续新启动的子代理。
- 提供统一的诊断存储统计、清理和保护：可阻止 Trace 日志持续写盘，并在 macOS 上限制待处理崩溃报告异常堆积，帮助避免诊断数据长期占用磁盘。
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
