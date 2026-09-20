# Codey Plugin SDK

此 SDK 用于独立开发 Codey 原生 Rust 插件，不依赖 CPA 或 Codex 插件市场。插件编译为 `cdylib`，通过 `export_plugin!` 同时导出 `codey_plugin_entry_v1` 和可选的 `codey_plugin_entry_with_context_v1`。Rust trait 仅在插件内部使用，动态库边界采用 ABI v1 的 C 布局函数表和字节缓冲区，返回内存由插件自己的释放函数回收。

实现 `Plugin::create` 与 `Plugin::invoke` 后即可使用导出宏。宿主串行调用同一个实例；构造函数和回调应及时返回。自行创建的后台任务必须在实例销毁前停止。SDK 在展开模式下捕获 Rust panic；无法隔离段错误、进程退出或死循环，因此只应安装可信插件。

## 安装包

`.codey-plugin` 是包含 `manifest.json`、动态库及根目录 `config.json` 配置模板的 ZIP。每个包对应一个操作系统和架构。manifest 包含插件 ID、名称、semver 版本、`abiVersion`、`platform`、`arch`、`entry`、`librarySha256`、`capabilities` 和 `headerNames`。平台使用 `macos`、`windows` 或 `linux`，架构使用 Rust 名称，例如 `aarch64` 或 `x86_64`。

导入只检查文件，不执行动态库；用户启用后才加载。SHA-256 仅校验完整性，不证明发布者身份。安装版本和配置独立保存；已启用实例保持原配置和权限，更新后需停用再启用或重启 Codey。动态库映射保留到进程退出，以避免卸载后仍有回调访问代码；Windows 可能需要重启后才能删除曾加载的 DLL。

## 插件目录与持久数据

每个插件使用 Codey 用户状态目录下的 `codey-plugins/installed/<plugin-id>/`。程序制品位于 `versions/<version-uuid>/`，持久数据位于 `data/`，日志位于 `logs/`，唯一运行配置为插件目录中的 `config.json`。首次安装复制包内模板，升级保留已有配置。旧版直接位于插件目录中的版本制品仍可加载。配置及启用状态由宿主统一管理。

覆盖 `Plugin::create_with_context(config, context)` 可获取 `PluginContext`，包含绝对路径 `plugin_dir`、`data_dir`、`log_dir` 和 `plugin_id`。宿主优先调用新入口，输入为 `{"config":...,"context":{"pluginId":...,"pluginDir":...,"dataDir":...,"logDir":...}}`；原入口仍只接收原配置。默认实现调用 `create(config)`，已有插件无需改动。宿主不会修改全进程工作目录或环境变量。

升级、停用、重新启用及重启均保留 data 和 logs。卸载默认保留配置、数据与日志，重装相同 ID 后继续使用；选择清理数据才删除插件目录。仍在执行或销毁的实例会阻止卸载，避免清理后回调重新写入。库映射可能使 Windows 文件清理需要退出 Codey 后再进行，清理失败会明确返回残留路径。

使用 `context.data_dir.join("state.json")` 保存业务数据；应使用固定文件名、限制文件大小，并自行处理并发和原子更新。`context.log("fixed_event")` 写入 `logs/plugin.log`，宿主生命周期事件写入 `logs/host.log`。每种日志最多保留当前文件与一个备份，每个文件上限 1 MiB，单条事件上限 4096 字节；日志 helper 不会重建缺失目录。避免在每次路由请求成功时同步写日志，不写入配置、凭据或请求内容。日志 helper 使用同目录的锁文件协调跨版本实例及进程的写入和轮转；无法取得锁时返回错误，由调用者决定忽略或稍后重试。该约定要求所有写入方使用此接口。

这些目录是可信原生插件遵守的存储约定，不构成文件系统沙箱；宿主无法阻止插件自行访问其他目录或绕过日志轮转。

## 配置与扩展

配置文件使用严格的 UTF-8 JSON 对象，最多 1 MiB，不支持 `//` 或 JSONC 注释。配置编辑器按文件中的对象与数组展示已有字段，说明位于配置项上方且不可选中或编辑，字段名和结构固定，只能修改叶子值。保存仅替换修改过的值，保留其余原文，并检查文件是否被其他编辑器修改。新增字段、调整结构或修正无效文件需在文件中完成后重新加载。插件不再提供配置 schema 或 HTML 配置页；字段默认值写入模板，业务约束由插件初始化时校验。配置保存在当前用户的私有目录中，目前没有独立密钥存储服务。

任意对象层级（包括数组内的对象）可添加 `_comments` 对象，其键名不限，每项值必须是说明字符串，例如 `"_comments": {"value": "请求头的值"}`。宿主统一校验，并在传给插件前移除这些注释字段；文件原文仍保留说明，插件无需自行处理。只有精确的 `_comments` 字段受此约定约束，其他下划线字段及字符串内容保持原样。只修改说明无需重新启用；修改业务参数后，运行中的插件需重新启用才生效。

显示说明时优先使用同级字段名，再查找祖先对象的点分隔字段名；数组路径省略下标，例如 `stateConfigs.model` 可说明每项的 `model`。真实的同名含点字段优先，存在歧义时应把说明放在嵌套对象自身的 `_comments` 中。没有对应字段的说明保留在文件中，不生成可编辑项。

首版支持 `request.beforeSend`。此回调收到 `params.metadata`（请求 ID、线路 ID、账号句柄、模型和协议）及 `params.headers`，后者只包含 manifest 的 `headerNames` 声明的现有字段。回调返回 `{"headers":[{"name":"x-example","value":"value"}]}`，value 为 null 表示移除。认证、传输控制及 Codey 内部请求头不可修改。请求体不会发送给插件；核心的路由提示规范化仍在回调后执行。

多个插件按 ID 排序执行，后一个可以看到前一个对相同授权字段的修改。返回值整体验证后才应用；回调失败的实例退出活动集合，原请求继续处理，用户可在管理界面查看错误并重新启用。其他方法由插件自行定义，可通过 `invoke_codey_plugin` 管理命令调用。

## 示例

`examples/plugins/header-demo` 演示配置、`ping` 方法和请求头扩展。在仓库根目录执行：

```sh
cargo build -p codey-plugin-header-demo
python3 scripts/package-plugin.py --library target/debug/libcodey_plugin_header_demo.dylib --config examples/plugins/header-demo/config.json --output /tmp/header-demo.codey-plugin --id dev.codey.header-demo --name 请求头示例 --version 0.1.0 --header x-plugin-demo
```

示例命令的动态库路径适用于 macOS；其他平台使用对应扩展名。若设置了 Cargo target-dir，应替换制品路径。打包工具不覆盖已有输出，省略 `--config` 时写入空对象模板。示例只添加测试请求头，首次验证建议使用本地模拟上游。
