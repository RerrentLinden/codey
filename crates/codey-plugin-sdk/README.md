# Codey Plugin SDK

此 SDK 用于独立开发 Codey 原生 Rust 插件，不依赖 CPA 或 Codex 插件市场。插件编译为 `cdylib`，通过 `export_plugin!` 同时导出 `codey_plugin_entry_v1` 和可选的 `codey_plugin_entry_with_context_v1`。Rust trait 仅在插件内部使用，动态库边界采用 ABI v1 的 C 布局函数表和字节缓冲区，返回内存由插件自己的释放函数回收。

实现 `Plugin::create` 与 `Plugin::invoke` 后即可使用导出宏。宿主串行调用同一个实例；构造函数和回调应及时返回。自行创建的后台任务必须在实例销毁前停止。SDK 在展开模式下捕获 Rust panic；无法隔离段错误、进程退出或死循环，因此只应安装可信插件。

## 安装包

`.codey-plugin` 是包含 `manifest.json`、动态库及配置 schema 的 ZIP。每个包对应一个操作系统和架构。manifest 包含插件 ID、名称、semver 版本、`abiVersion`、`platform`、`arch`、`entry`、`librarySha256`、`configSchema`、`capabilities` 和 `headerNames`。平台使用 `macos`、`windows` 或 `linux`，架构使用 Rust 名称，例如 `aarch64` 或 `x86_64`。

导入只检查文件，不执行动态库；用户启用后才加载。SHA-256 仅校验完整性，不证明发布者身份。安装版本和配置独立保存；已启用实例保持原配置和权限，更新后需停用再启用或重启 Codey。动态库映射保留到进程退出，以避免卸载后仍有回调访问代码；Windows 可能需要重启后才能删除曾加载的 DLL。

## 插件目录与持久数据

每个插件使用 Codey 用户状态目录下的 `codey-plugins/installed/<plugin-id>/`。程序制品位于 `versions/<version-uuid>/`，持久数据位于 `data/`，日志位于 `logs/`。旧版直接位于插件目录中的版本制品仍可加载。配置及启用状态由宿主统一保存，不应由插件直接修改。

覆盖 `Plugin::create_with_context(config, context)` 可获取 `PluginContext`，包含绝对路径 `plugin_dir`、`data_dir`、`log_dir` 和 `plugin_id`。宿主优先调用新入口，输入为 `{"config":...,"context":{"pluginId":...,"pluginDir":...,"dataDir":...,"logDir":...}}`；原入口仍只接收原配置。默认实现调用 `create(config)`，已有插件无需改动。宿主不会修改全进程工作目录或环境变量。

升级、停用、重新启用及重启均保留 data 和 logs。卸载默认保留配置、数据与日志，重装相同 ID 后继续使用；选择清理数据才删除插件目录。仍在执行或销毁的实例会阻止卸载，避免清理后回调重新写入。库映射可能使 Windows 文件清理需要退出 Codey 后再进行，清理失败会明确返回残留路径。

使用 `context.data_dir.join("state.json")` 保存业务数据；应使用固定文件名、限制文件大小，并自行处理并发和原子更新。`context.log("fixed_event")` 写入 `logs/plugin.log`，宿主生命周期事件写入 `logs/host.log`。每种日志最多保留当前文件与一个备份，每个文件上限 1 MiB，单条事件上限 4096 字节；日志 helper 不会重建缺失目录。避免在每次路由请求成功时同步写日志，不写入配置、凭据或请求内容。日志 helper 使用同目录的锁文件协调跨版本实例及进程的写入和轮转；无法取得锁时返回错误，由调用者决定忽略或稍后重试。该约定要求所有写入方使用此接口。

这些目录是可信原生插件遵守的存储约定，不构成文件系统沙箱；宿主无法阻止插件自行访问其他目录或绕过日志轮转。

## 配置与扩展

配置根类型为 object。支持 object、array、string、boolean、integer、number、null，以及 required、additionalProperties 布尔值、enum、数值范围、字符串长度、数组长度和 default。未支持的约束会被拒绝。配置保存在当前用户的私有目录中；目前没有独立密钥存储服务。

首版支持 `request.beforeSend`。此回调收到 `params.metadata`（请求 ID、线路 ID、账号句柄、模型和协议）及 `params.headers`，后者只包含 manifest 的 `headerNames` 声明的现有字段。回调返回 `{"headers":[{"name":"x-example","value":"value"}]}`，value 为 null 表示移除。认证、传输控制及 Codey 内部请求头不可修改。请求体不会发送给插件；核心的路由提示规范化仍在回调后执行。

多个插件按 ID 排序执行，后一个可以看到前一个对相同授权字段的修改。返回值整体验证后才应用；回调失败的实例退出活动集合，原请求继续处理，用户可在管理界面查看错误并重新启用。其他方法由插件自行定义，可通过 `invoke_codey_plugin` 管理命令调用。

## 示例

`examples/plugins/header-demo` 演示配置、`ping` 方法和请求头扩展。在仓库根目录执行：

```sh
cargo build -p codey-plugin-header-demo
python3 scripts/package-plugin.py --library target/debug/libcodey_plugin_header_demo.dylib --schema examples/plugins/header-demo/config.schema.json --output /tmp/header-demo.codey-plugin --id dev.codey.header-demo --name 请求头示例 --version 0.1.0 --header x-plugin-demo
```

示例命令的动态库路径适用于 macOS；其他平台使用对应扩展名。若设置了 Cargo target-dir，应替换制品路径。打包工具不覆盖已有输出。示例只添加测试请求头，首次验证建议使用本地模拟上游。

## 声明式配置表单

插件提供 `config.schema.json`，Codey 的统一表单据此呈现输入框、开关、枚举选择、嵌套对象以及可增删的数组。`title` 和 `description` 提供名称及说明，`required` 标记必填字段，显式 `default` 用于初始化缺失值。可选字段可以清除，空数字输入会阻止保存。前端提供字段校验反馈，后端仍作最终校验；保存不会改变当前运行实例。

这是 JSON Schema 子集，非完整实现：不支持 `$ref`、条件分支、联合类型、自定义控件或任意扩展属性。默认表单不执行插件提供的 HTML 或 JavaScript；需要定制布局和交互时可声明下述 HTML 配置页。自由对象或无法直接呈现的结构可使用 JSON 辅助编辑。已有未知字段保留，`additionalProperties: false` 时必须删除这些字段后才能保存。配置中的敏感文本目前不具备专门的密钥输入及存储机制。

例如需要代理池的插件可声明以下配置；这仅展示通用表单结构，不为请求头示例增加代理功能：

```json
{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "proxies": {
      "type": "array",
      "title": "代理池",
      "default": [],
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
          "url": { "type": "string", "title": "代理地址", "minLength": 1 },
          "enabled": { "type": "boolean", "title": "启用", "default": true },
          "weight": { "type": "integer", "title": "权重", "minimum": 1, "default": 1 }
        }
      }
    }
  }
}
```

## 自定义 HTML 配置页

可在 manifest 中增加可选的 `configUi`，未声明时继续使用统一配置表单，原生 ABI 不变：

```json
{
  "configUi": {
    "type": "html",
    "entry": "ui/config.html",
    "sha256": "配置页文件内容的小写SHA-256"
  }
}
```

配置页必须为单文件 UTF-8 HTML，最多 1 MiB，CSS 和 JavaScript 内嵌。入口使用包内相对路径，禁止符号链接、特殊文件及目录穿越。打包工具通过 `--config-ui examples/plugins/header-demo/ui/config.html` 自动加入 `ui/config.html` 并计算摘要；不传该参数时生成原有格式。安装检查及读取已安装配置页均校验摘要；摘要只用于内容完整性检查。列表只返回配置页元数据，打开配置时通过 `get_codey_plugin_config_ui` 读取当前安装版本，不加载动态库。

安装与检查不执行 HTML。打开配置弹窗时，页面 JavaScript 在仅启用 `allow-scripts`、未启用 `allow-same-origin` 的 sandbox iframe 中执行。页面通过独立 MessageChannel 交换本插件的配置草稿，不能调用 Codey 全局 API；宿主统一保存并再次使用配置 schema 校验。因此即使提供 HTML，`configSchema` 仍必需，保存后已启用的原生实例仍需重新启用才能应用配置。

宿主注入的接口为：

```js
window.CodeyPluginConfig.onInit(({ config, theme }) => {
  // config 为当前草稿，theme 为 light 或 dark。
  renderSettings(config, theme);
});
window.CodeyPluginConfig.setConfig({ value: "edited-value" });
window.CodeyPluginConfig.setValidity(true);
// 字段无效时阻止宿主保存，并显示原因。
window.CodeyPluginConfig.setValidity(false, "请填写有效配置");
```

页面应在初始化完成后操作草稿，并通过宿主的保存按钮提交；接口不提供文件读取、网络请求、原生方法调用或其他插件的数据。CSP 限制网络资源、表单提交及子框架，页面不应依赖远程脚本或外部样式。此隔离针对配置页，不为原生动态库提供沙箱，也不承诺 CSP 能阻止 iframe 自身的所有导航。
