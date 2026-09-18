use codey_plugin_sdk::{
    Plugin, PluginContext,
    serde_json::{Value, json},
};

struct HeaderDemo {
    value: String,
    context: Option<PluginContext>,
}

impl Plugin for HeaderDemo {
    fn create(config: Value) -> Result<Self, String> {
        let value = config
            .get("value")
            .and_then(Value::as_str)
            .ok_or("缺少 value 配置")?;
        if value.is_empty()
            || value.len() > 128
            || !value.is_ascii()
            || value.bytes().any(|b| b < 32 || b == 127)
        {
            return Err("value 必须是 1 到 128 字节的可打印 ASCII 文本".into());
        }
        Ok(Self {
            value: value.into(),
            context: None,
        })
    }

    fn create_with_context(config: Value, context: PluginContext) -> Result<Self, String> {
        let mut plugin = Self::create(config)?;
        context.log("header_demo_created")?;
        plugin.context = Some(context);
        Ok(plugin)
    }

    fn invoke(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "request.beforeSend" => {
                Ok(json!({"headers":[{"name":"x-plugin-demo", "value":self.value}]}))
            }
            "ping" => Ok(json!({"plugin":"header-demo","value":self.value,"params":params})),
            "storage.write" => {
                let context = self.context.as_ref().ok_or("当前宿主没有提供插件目录")?;
                // Fixed filename, no caller-controlled paths, and a bounded payload.
                let bytes =
                    codey_plugin_sdk::serde_json::to_vec(&params).map_err(|e| e.to_string())?;
                if bytes.len() > 4096 {
                    return Err("示例数据最多 4096 字节".into());
                }
                std::fs::write(context.data_dir.join("example.json"), bytes)
                    .map_err(|e| e.to_string())?;
                context.log("example_saved")?;
                Ok(Value::Null)
            }
            "storage.read" => {
                let context = self.context.as_ref().ok_or("当前宿主没有提供插件目录")?;
                let bytes = std::fs::read(context.data_dir.join("example.json"))
                    .map_err(|e| e.to_string())?;
                codey_plugin_sdk::serde_json::from_slice(&bytes).map_err(|e| e.to_string())
            }
            "storage.context" => {
                codey_plugin_sdk::serde_json::to_value(&self.context).map_err(|e| e.to_string())
            }
            _ => Err(format!("未知方法: {method}")),
        }
    }
}

codey_plugin_sdk::export_plugin!(HeaderDemo);
