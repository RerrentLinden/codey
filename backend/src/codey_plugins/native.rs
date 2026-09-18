use codey_plugin_sdk::{
    ABI_VERSION, Buffer, EntryPoint, MAX_MESSAGE_BYTES, PluginApiV1, PluginContext,
};
use serde_json::Value;
use std::{
    ffi::c_void,
    path::Path,
    sync::{Arc, Weak},
};

pub struct Native {
    api: PluginApiV1,
    instance: *mut c_void,
    // Dropped after destroy returns, including when the last Arc is being destroyed.
    lifetime: Arc<()>,
}

// ABI v1 requires a Send instance. The host always serializes calls with a mutex.
unsafe impl Send for Native {}

impl Native {
    pub fn load(path: &Path, config: Value, context: PluginContext) -> Result<Self, String> {
        // Loading runs native initializers. Only call after explicit user enablement.
        let library = unsafe { libloading::Library::new(path) }
            .map_err(|e| format!("无法加载动态库: {e}"))?;
        // Even entry-point/initialization failures may leave native callbacks running.
        // Keep every successfully opened mapping until process exit.
        let library = Box::leak(Box::new(library));
        let (entry, input) =
            match unsafe { library.get::<EntryPoint>(b"codey_plugin_entry_with_context_v1\0") } {
                Ok(entry) => (
                    entry,
                    serde_json::json!({"config": config, "context": context}),
                ),
                Err(_) => (
                    unsafe { library.get::<EntryPoint>(b"codey_plugin_entry_v1\0") }
                        .map_err(|e| format!("缺少 ABI 入口: {e}"))?,
                    config,
                ),
            };
        let api = unsafe { entry() };
        if api.is_null() {
            return Err("插件返回了空 ABI 表".into());
        }
        let abi_version = unsafe { (*api).abi_version };
        let struct_size = unsafe { (*api).struct_size };
        if abi_version != ABI_VERSION || struct_size as usize != std::mem::size_of::<PluginApiV1>()
        {
            return Err("插件 ABI 表不兼容".into());
        }
        let api = unsafe { *api };
        let bytes = encode(&input)?;
        let mut instance = std::ptr::null_mut();
        let mut output = Buffer::default();
        let status =
            unsafe { (api.create)(bytes.as_ptr(), bytes.len(), &mut instance, &mut output) };
        let result = decode(&api, status, output);
        if let Err(error) = result {
            if !instance.is_null() {
                unsafe {
                    (api.destroy)(instance);
                }
            }
            return Err(error);
        }
        if instance.is_null() {
            return Err("插件初始化未返回实例".into());
        }
        Ok(Self {
            api,
            instance,
            lifetime: Arc::new(()),
        })
    }

    pub fn lifetime(&self) -> Weak<()> {
        Arc::downgrade(&self.lifetime)
    }

    pub fn invoke(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if method.is_empty() || method.len() > 128 {
            return Err("插件方法名无效".into());
        }
        let input = encode(&serde_json::json!({"method":method,"params":params}))?;
        let mut output = Buffer::default();
        let status =
            unsafe { (self.api.invoke)(self.instance, input.as_ptr(), input.len(), &mut output) };
        decode(&self.api, status, output)
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            (self.api.destroy)(self.instance);
        }
    }
}

fn encode(value: &Value) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("插件消息超过 1 MiB".into());
    }
    Ok(bytes)
}

fn decode(api: &PluginApiV1, status: i32, output: Buffer) -> Result<Value, String> {
    let result = if output.data.is_null() || output.len == 0 || output.len > MAX_MESSAGE_BYTES {
        Err("插件输出指针或长度无效".into())
    } else {
        serde_json::from_slice::<Value>(unsafe {
            std::slice::from_raw_parts(output.data, output.len)
        })
        .map_err(|e| format!("插件输出不是 JSON: {e}"))
    };
    unsafe {
        (api.free_buffer)(output);
    }
    let value = result?;
    if status != 0 {
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("插件返回错误")
            .to_owned());
    }
    Ok(value)
}
