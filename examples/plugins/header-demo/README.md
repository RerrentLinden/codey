# Codey 请求头示例插件

这是独立的 Rust cdylib 插件，依赖 Codey SDK，不依赖 CPA。
仅从可信来源加载原生插件：它拥有宿主进程权限，不能视为沙箱。

从仓库根目录构建 macOS 示例：

```sh
cargo build -p codey-plugin-header-demo
python3 scripts/package-plugin.py \
  --library target/debug/libcodey_plugin_header_demo.dylib \
  --schema examples/plugins/header-demo/config.schema.json \
  --output /tmp/header-demo.codey-plugin \
  --id dev.codey.header-demo --name HeaderDemo --version 0.1.0 \
  --header x-plugin-demo
```

Linux 改用 `.so`；Windows 使用 `codey_plugin_header_demo.dll`。跨平台构建时传入实际制品的 `--platform` 与 `--arch`，不能直接使用打包机器的平台信息。

导入后默认禁用。启用后只在路由扩展点修改声明的 `X-Plugin-Demo`，可通过 `ping` 方法检查配置。配置更新和版本更新会提示重新启用，运行中的实例继续使用自己的配置快照。

示例通过 `create_with_context` 获取插件独立目录，初始化事件写入 `logs/plugin.log`。调用 `storage.write` 可把最多 4096 字节的 JSON 参数写入 `data/example.json`，`storage.read` 读取它，`storage.context` 返回宿主传入的目录。升级和重新启用继续使用同一份数据；保留数据卸载后重装也可恢复。旧宿主仍能通过原 ABI 使用请求头及 ping 功能，但没有存储上下文。

SDK 宏在动态库内部包装 Rust trait，跨库接口是 ABI v1 的 C 函数表。输入只在调用期间有效；输出必须使用返回该缓冲区的插件释放函数。插件应在析构时取消并结束自己创建的任务。宿主不会卸载已映射的动态库，也不能中断卡死的原生调用。
