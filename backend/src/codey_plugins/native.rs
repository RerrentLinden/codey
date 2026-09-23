use codey_plugin_sdk::{
    ABI_VERSION, Buffer, EntryPoint, MAX_MESSAGE_BYTES, PluginApiV1, PluginContext,
};
use serde_json::Value;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
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
        let entry = unsafe { library.get::<EntryPoint>(b"codey_plugin_entry_v1\0") }
            .map_err(|e| format!("缺少 ABI 入口 codey_plugin_entry_v1: {e}"))?;
        let input = serde_json::json!({"config": config, "context": context});
        let api = catch_plugin_panic("插件初始化发生 panic", || unsafe { entry() })?;
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
        let status = match catch_plugin_panic("插件初始化发生 panic", || unsafe {
            (api.create)(bytes.as_ptr(), bytes.len(), &mut instance, &mut output)
        }) {
            Ok(status) => status,
            Err(error) => {
                destroy_instance(&api, instance);
                release_buffer(&api, output);
                return Err(error);
            }
        };
        let result = decode(&api, status, output);
        if let Err(error) = result {
            destroy_instance(&api, instance);
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
        let status = match catch_plugin_panic("插件调用发生 panic", || unsafe {
            (self.api.invoke)(self.instance, input.as_ptr(), input.len(), &mut output)
        }) {
            Ok(status) => status,
            Err(error) => {
                release_buffer(&self.api, output);
                return Err(error);
            }
        };
        decode(&self.api, status, output)
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        destroy_instance(&self.api, self.instance);
    }
}

fn catch_plugin_panic<T>(message: &str, action: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(action)).map_err(|_| message.to_owned())
}

fn destroy_instance(api: &PluginApiV1, instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    let destroy = api.destroy;
    let _ = catch_plugin_panic("插件销毁发生 panic", || unsafe { destroy(instance) });
}

fn release_buffer(api: &PluginApiV1, output: Buffer) {
    // 长度为 0 却带非空指针违反 ABI：按 len 释放会以零长度 Layout 释放真实分配
    // 并损坏堆。宿主无从得知真实大小，因此宁可保留这一份泄漏（插件进程退出即
    // 回收），也不释放大小未知的指针。
    if output.len == 0 && !output.data.is_null() {
        return;
    }
    let free = api.free_buffer;
    let _ = catch_plugin_panic("插件释放输出发生 panic", || unsafe { free(output) });
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
    release_buffer(api, output);
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
