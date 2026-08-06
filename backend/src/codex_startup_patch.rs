#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use anyhow::Result;

const PATCH_RESULT: &str = "codey-startup-patch-installed-v20";
const PET_HARD_DISABLE_STATUS_EXPRESSION: &str = r#"
globalThis.__CODEY_PET_HARD_DISABLE_STATUS__ ?? { phase: "pending", message: "" }
"#;
const CLOSE_INSPECTOR_EXPRESSION: &str = r#"
(() => {
  setImmediate(() => {
    try { process.getBuiltinModule("inspector").close(); } catch {}
  });
  return "codey-inspector-close-scheduled";
})()
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchOptions {
    pub disable_pet: bool,
    pub disable_voice: bool,
    pub fast_codex_startup: bool,
}

pub fn inspector_argument(port: u16) -> String {
    format!("--inspect-brk=127.0.0.1:{port}")
}

const STARTUP_PATCH_TEMPLATE: &str = concat!("\n", include_str!("codex_startup_patch.js"));

fn patch_expression(options: PatchOptions) -> String {
    let error_logger_executable = match std::env::current_exe() {
        Ok(path) => serde_json::to_string(&path.to_string_lossy().to_string())
            .expect("error logger executable path should serialize"),
        Err(error) => {
            crate::error_log::record_failure(
                "patch_failed",
                "resolve_error_log_helper",
                error.to_string(),
                serde_json::json!({}),
            );
            "\"\"".to_string()
        }
    };
    STARTUP_PATCH_TEMPLATE
        .replace(
            "\"__CODEY_ERROR_LOGGER_EXECUTABLE__\"",
            &error_logger_executable,
        )
        .replace(
            "__DISABLE_PET__",
            if options.disable_pet { "true" } else { "false" },
        )
        .replace(
            "__DISABLE_VOICE__",
            if options.disable_voice {
                "true"
            } else {
                "false"
            },
        )
        .replace(
            "__FAST_CODEX_STARTUP__",
            if options.fast_codex_startup {
                "true"
            } else {
                "false"
            },
        )
}

pub fn reserve_loopback_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

pub async fn install(port: u16, options: PatchOptions) -> Result<()> {
    let websocket_url = wait_for_inspector(port).await?;
    let expression = patch_expression(options);
    let verify_pet_hard_disable = options.disable_pet;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        install_over_websocket(&websocket_url, &expression, verify_pet_hard_disable),
    )
    .await
    .map_err(|_| {
        if verify_pet_hard_disable {
            anyhow::anyhow!("Codex 启动补丁已注入，但等待宠物 manager 硬禁用生效超时")
        } else {
            anyhow::anyhow!("Codex 启动补丁调试会话超时")
        }
    })??;
    Ok(())
}

async fn wait_for_inspector(port: u16) -> Result<String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_millis(750))
        .build()?;
    let endpoint = format!("http://127.0.0.1:{port}/json/list");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut last_error = "调试端口尚未响应".to_string();
    let mut retry_delay = std::time::Duration::from_millis(20);

    while tokio::time::Instant::now() < deadline {
        match client.get(&endpoint).send().await {
            Ok(response) if response.status().is_success() => {
                match response.json::<Vec<serde_json::Value>>().await {
                    Ok(targets) => {
                        if let Some(url) = targets.iter().find_map(|target| {
                            target
                                .get("webSocketDebuggerUrl")
                                .and_then(serde_json::Value::as_str)
                        }) {
                            return Ok(url.to_string());
                        }
                        last_error = "调试端口没有可连接的目标".to_string();
                    }
                    Err(error) => last_error = error.to_string(),
                }
            }
            Ok(response) => last_error = format!("调试端口返回 HTTP {}", response.status()),
            Err(error) => last_error = error.to_string(),
        }
        tokio::time::sleep(retry_delay).await;
        retry_delay = std::cmp::min(
            retry_delay.saturating_mul(2),
            std::time::Duration::from_millis(100),
        );
    }

    anyhow::bail!("等待 Codex 启动补丁超时：{last_error}")
}

async fn install_over_websocket(
    websocket_url: &str,
    expression: &str,
    verify_pet_hard_disable: bool,
) -> Result<()> {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    let (mut socket, _) = tokio_tungstenite::connect_async(websocket_url).await?;
    send_command(&mut socket, 1, "Runtime.enable", serde_json::json!({})).await?;
    send_command(&mut socket, 2, "Debugger.enable", serde_json::json!({})).await?;

    let mut runtime_enabled = false;
    let mut debugger_enabled = false;
    let mut continued = false;
    let mut evaluation_sent = false;
    let mut next_command_id = 6_u64;
    let mut pet_status_command_id = None;
    let mut close_inspector_command_id = None;

    while let Some(message) = socket.next().await {
        let message = message?;
        let text = match message {
            Message::Text(text) => text,
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
                continue;
            }
            Message::Close(_) if close_inspector_command_id.is_some() => return Ok(()),
            Message::Close(_) => anyhow::bail!("Codex 启动补丁调试连接提前关闭"),
        };
        let payload: serde_json::Value = serde_json::from_str(text.as_ref())?;

        match payload.get("id").and_then(serde_json::Value::as_u64) {
            Some(1) => {
                ensure_protocol_success(&payload, "Runtime.enable")?;
                runtime_enabled = true;
            }
            Some(2) => {
                ensure_protocol_success(&payload, "Debugger.enable")?;
                debugger_enabled = true;
            }
            Some(3) => {
                ensure_protocol_success(&payload, "Runtime.runIfWaitingForDebugger")?;
            }
            Some(4) => {
                ensure_protocol_success(&payload, "Debugger.evaluateOnCallFrame")?;
                if let Some(exception) = payload
                    .get("result")
                    .and_then(|result| result.get("exceptionDetails"))
                {
                    anyhow::bail!("Codex 启动补丁执行异常：{exception}");
                }
                let value = payload
                    .pointer("/result/result/value")
                    .and_then(serde_json::Value::as_str);
                if value != Some(PATCH_RESULT) {
                    anyhow::bail!("Codex 启动补丁未返回预期状态");
                }
                send_command(&mut socket, 5, "Debugger.resume", serde_json::json!({})).await?;
            }
            Some(5) => {
                ensure_protocol_success(&payload, "Debugger.resume")?;
                if verify_pet_hard_disable {
                    let command_id = next_command_id;
                    next_command_id += 1;
                    pet_status_command_id = Some(command_id);
                    send_command(
                        &mut socket,
                        command_id,
                        "Runtime.evaluate",
                        serde_json::json!({
                            "expression": PET_HARD_DISABLE_STATUS_EXPRESSION,
                            "returnByValue": true,
                            "silent": false,
                        }),
                    )
                    .await?;
                    continue;
                }
                let _ = socket.close(None).await;
                return Ok(());
            }
            Some(command_id) if pet_status_command_id == Some(command_id) => {
                ensure_protocol_success(&payload, "Runtime.evaluate pet hard-disable status")?;
                if let Some(exception) = payload
                    .get("result")
                    .and_then(|result| result.get("exceptionDetails"))
                {
                    anyhow::bail!("读取 Codex 宠物硬禁用状态异常：{exception}");
                }
                let status = payload
                    .pointer("/result/result/value")
                    .and_then(serde_json::Value::as_object)
                    .ok_or_else(|| anyhow::anyhow!("Codex 宠物硬禁用状态返回格式无效"))?;
                let phase = status
                    .get("phase")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("Codex 宠物硬禁用状态缺少 phase"))?;
                match phase {
                    "removed" => {
                        pet_status_command_id = None;
                        let command_id = next_command_id;
                        close_inspector_command_id = Some(command_id);
                        send_command(
                            &mut socket,
                            command_id,
                            "Runtime.evaluate",
                            serde_json::json!({
                                "expression": CLOSE_INSPECTOR_EXPRESSION,
                                "returnByValue": true,
                                "silent": true,
                            }),
                        )
                        .await?;
                    }
                    "error" => {
                        let message = status
                            .get("message")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown Codex main bundle patch error");
                        anyhow::bail!("Codex 宠物 manager 硬禁用失败：{message}");
                    }
                    "pending" => {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                        let command_id = next_command_id;
                        next_command_id += 1;
                        pet_status_command_id = Some(command_id);
                        send_command(
                            &mut socket,
                            command_id,
                            "Runtime.evaluate",
                            serde_json::json!({
                                "expression": PET_HARD_DISABLE_STATUS_EXPRESSION,
                                "returnByValue": true,
                                "silent": false,
                            }),
                        )
                        .await?;
                    }
                    other => {
                        anyhow::bail!("Codex 宠物硬禁用状态不可识别：{other}");
                    }
                }
            }
            Some(command_id) if close_inspector_command_id == Some(command_id) => {
                ensure_protocol_success(&payload, "Runtime.evaluate close inspector")?;
                if let Some(exception) = payload
                    .get("result")
                    .and_then(|result| result.get("exceptionDetails"))
                {
                    anyhow::bail!("关闭 Codex 启动 Inspector 异常：{exception}");
                }
                let _ = socket.close(None).await;
                return Ok(());
            }
            _ => {}
        }

        if runtime_enabled && debugger_enabled && !continued {
            continued = true;
            send_command(
                &mut socket,
                3,
                "Runtime.runIfWaitingForDebugger",
                serde_json::json!({}),
            )
            .await?;
        }

        if payload.get("method").and_then(serde_json::Value::as_str) == Some("Debugger.paused")
            && !evaluation_sent
        {
            let frame_id = payload
                .pointer("/params/callFrames/0/callFrameId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Codex 启动补丁没有收到可用的调用栈"))?;
            evaluation_sent = true;
            send_command(
                &mut socket,
                4,
                "Debugger.evaluateOnCallFrame",
                serde_json::json!({
                    "callFrameId": frame_id,
                    "expression": expression,
                    "returnByValue": true,
                    "silent": false,
                }),
            )
            .await?;
        }
    }

    anyhow::bail!("Codex 启动补丁调试连接未返回执行结果")
}

async fn send_command<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let message = serde_json::json!({
        "id": id,
        "method": method,
        "params": params,
    });
    socket
        .send(Message::Text(message.to_string().into()))
        .await?;
    Ok(())
}

fn ensure_protocol_success(payload: &serde_json::Value, method: &str) -> Result<()> {
    if let Some(error) = payload.get("error") {
        anyhow::bail!("{method} 失败：{error}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspector_is_loopback_only_and_pauses_before_startup() {
        assert_eq!(inspector_argument(19321), "--inspect-brk=127.0.0.1:19321");
    }

    #[test]
    fn patch_result_is_stable_for_launch_status_validation() {
        assert_eq!(PATCH_RESULT, "codey-startup-patch-installed-v20");
    }

    #[test]
    fn patch_expression_can_hard_disable_pet_with_platform_gated_windows_optimizations() {
        let expression = patch_expression(PatchOptions {
            disable_pet: true,
            disable_voice: false,
            fast_codex_startup: true,
        });

        assert!(expression.contains("const disablePet = true"));
        assert!(
            expression
                .contains("const disableWindowsOptimizations = process.platform === \"win32\"")
        );
        assert!(expression.contains("const disableMicro = disableWindowsOptimizations"));
        assert!(expression.contains("CodeyPetBlockedBrowserWindow"));
        assert!(expression.contains("patchCodexRendererResponse"));
        assert!(expression.contains("restoreNativeModelAndSpeedControls: true"));
        assert!(expression.contains("avatar-overlay-composition-surface-preload"));
        assert!(expression.contains("avatar(?:-|_)overlay"));
        assert!(expression.contains("__CODEY_DISABLED_PET_MANAGER__"));
        assert!(expression.contains("getVisibleNativePetWebContents"));
        assert!(expression.contains("__CODEY_PET_HARD_DISABLE_STATUS__"));
        assert!(expression.contains("petHardDisableStatus.phase = \"removed\""));
        assert!(expression.contains("guardAvatarOverlayLifecycle"));
        assert!(expression.contains("initialroute"));
        assert!(expression.contains("avatarOverlayDestroyedWindows"));
        assert!(expression.contains("if (!disablePet)"));
        let compile_position = expression
            .find("module._compile(source, filename);")
            .expect("main bundle compile call");
        let removal_confirmation_position = expression
            .find("globalThis.__CODEY_PET_MANAGER_SOURCE_REMOVED__ = true;")
            .expect("pet manager removal confirmation");
        assert!(compile_position < removal_confirmation_position);
        assert!(expression.contains("disableAppServerAnalytics: true"));
        assert!(expression.contains("get disableDesktopCesAnalytics()"));
        assert!(expression.contains("analytics.enabled=false"));
        assert!(expression.contains("reconcileExternalPluginState"));
        assert!(expression.contains("get throttleExternalPluginFocusReconcile()"));
        assert!(expression.contains("get disableAppStateHeartbeat()"));
        assert!(expression.contains("get optionalMainBundlePatchFailures()"));
        assert!(expression.contains("module._compile(source, filename)"));
        assert!(expression.contains("const fastCodexStartup = true"));
        assert!(expression.contains("Codey Statsig bootstrap timeout"));
        assert!(expression.contains("statsigBootstrapTimeoutMs = 1500"));
        assert!(expression.contains("default Chinese locale"));
        assert!(expression.contains("__CODEY_DEFAULT_CHINESE_LOCALE_RENDERER_PATCH__"));
        assert!(expression.contains("spawnSync"));
        assert!(expression.contains("--codey-record-error"));
        assert!(!expression.contains("\"__CODEY_ERROR_LOGGER_EXECUTABLE__\""));
    }

    #[test]
    fn windows_lag_patch_only_short_circuits_the_wmi_snapshot_worker() {
        let expression = patch_expression(PatchOptions {
            disable_pet: false,
            disable_voice: false,
            fast_codex_startup: true,
        });

        assert!(expression.contains("process.platform === \"win32\""));
        assert!(expression.contains("child-process-snapshot-worker\\.js"));
        assert!(expression.contains("CodeyDisabledWmiSnapshotWorker"));
        assert!(expression.contains("this.emit(\"message\", { type: \"ok\", value: [] })"));
        assert!(expression.contains("super(filename, {"));
    }

    #[test]
    fn fast_startup_bootstrap_cap_can_be_disabled() {
        let expression = patch_expression(PatchOptions {
            disable_pet: false,
            disable_voice: false,
            fast_codex_startup: false,
        });

        assert!(expression.contains("const fastCodexStartup = false"));
        assert!(!expression.contains("__FAST_CODEX_STARTUP__"));
    }

    #[test]
    fn voice_slimming_preserves_codex_initialization_services() {
        let expression = patch_expression(PatchOptions {
            disable_pet: false,
            disable_voice: true,
            fast_codex_startup: true,
        });

        assert!(expression.contains("const disableVoice = true"));
        assert!(!expression.contains("__CODEY_DISABLED_VOICE_MANAGER__"));
        assert!(!expression.contains("isVoiceHelper"));
        assert!(expression.contains("settings preload gate awaits responses"));
        assert!(expression.contains("options.appearance === \"globalDictation\""));
        assert!(expression.contains("CODEY_VOICE_DISABLED"));
    }

    #[test]
    fn automatic_lifecycle_patch_destroys_webviews_and_reclaims_execution_helpers() {
        let expression = patch_expression(PatchOptions {
            disable_pet: false,
            disable_voice: false,
            fast_codex_startup: true,
        });

        assert!(expression.contains("__CODEY_TEMP_WEBVIEW_LIFECYCLE__.close"));
        assert!(expression.contains("__CODEY_TEMP_WEBVIEW_LIFECYCLE__.track"));
        assert!(expression.contains("checkout-webview-presentation-changed"));
        assert!(expression.contains("__CODEY_INSTALL_EXECUTION_REAPER__"));
        assert!(expression.contains("const activeTurns = new Map()"));
        assert!(expression.contains("\"completed\""));
        assert!(expression.contains("\"aborted\""));
        assert!(expression.contains("reclaimAuthorizedVersion"));
        assert!(expression.contains("waitForReclaimBarrier"));
        assert!(!expression.contains("evictStaleTurns"));
        assert!(expression.contains("turnStateVersion"));
        assert!(expression.contains("codegraph\\.js\\s+serve"));
        assert!(expression.contains("node_repl"));
        assert!(expression.contains("handlers[\"child-process-kill\"]"));
    }

    #[tokio::test]
    async fn inspector_protocol_installs_stub_before_resuming() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();

            for expected_id in [1_u64, 2] {
                let message = socket.next().await.unwrap().unwrap();
                let Message::Text(text) = message else {
                    panic!("expected inspector command");
                };
                let command: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
                assert_eq!(command["id"], expected_id);
                socket
                    .send(Message::Text(
                        serde_json::json!({"id": expected_id, "result": {}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            }

            let message = socket.next().await.unwrap().unwrap();
            let Message::Text(text) = message else {
                panic!("expected runIfWaitingForDebugger");
            };
            let command: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
            assert_eq!(command["method"], "Runtime.runIfWaitingForDebugger");
            socket
                .send(Message::Text(
                    serde_json::json!({"id": 3, "result": {}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "method": "Debugger.paused",
                        "params": {
                            "callFrames": [{"callFrameId": "frame-1"}]
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();

            let message = socket.next().await.unwrap().unwrap();
            let Message::Text(text) = message else {
                panic!("expected evaluateOnCallFrame");
            };
            let command: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
            assert_eq!(command["method"], "Debugger.evaluateOnCallFrame");
            assert_eq!(command["params"]["callFrameId"], "frame-1");
            let expression = command["params"]["expression"].as_str().unwrap();
            assert!(expression.contains("@worklouder/device-kit-oai"));
            assert!(expression.contains("CodeyPetBlockedBrowserWindow"));
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "id": 4,
                        "result": {
                            "result": {
                                "type": "string",
                                "value": PATCH_RESULT
                            }
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();

            let message = socket.next().await.unwrap().unwrap();
            let Message::Text(text) = message else {
                panic!("expected Debugger.resume");
            };
            let command: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
            assert_eq!(command["method"], "Debugger.resume");
            socket
                .send(Message::Text(
                    serde_json::json!({"id": 5, "result": {}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();

            for (expected_id, phase) in [(6_u64, "pending"), (7_u64, "removed")] {
                let message = socket.next().await.unwrap().unwrap();
                let Message::Text(text) = message else {
                    panic!("expected pet hard-disable status query");
                };
                let command: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
                assert_eq!(command["id"], expected_id);
                assert_eq!(command["method"], "Runtime.evaluate");
                assert!(
                    command["params"]["expression"]
                        .as_str()
                        .unwrap()
                        .contains("__CODEY_PET_HARD_DISABLE_STATUS__")
                );
                socket
                    .send(Message::Text(
                        serde_json::json!({
                            "id": expected_id,
                            "result": {
                                "result": {
                                    "type": "object",
                                    "value": { "phase": phase, "message": "" }
                                }
                            }
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            }

            let message = socket.next().await.unwrap().unwrap();
            let Message::Text(text) = message else {
                panic!("expected inspector close command");
            };
            let command: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
            assert_eq!(command["id"], 8);
            assert_eq!(command["method"], "Runtime.evaluate");
            assert!(
                command["params"]["expression"]
                    .as_str()
                    .unwrap()
                    .contains("inspector\").close")
            );
            socket
                .send(Message::Text(
                    serde_json::json!({"id": 8, "result": {"result": {"value": "ok"}}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });

        let expression = patch_expression(PatchOptions {
            disable_pet: true,
            disable_voice: false,
            fast_codex_startup: true,
        });
        install_over_websocket(&format!("ws://{address}"), &expression, true)
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn inspector_protocol_fails_immediately_when_continue_is_rejected() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();

            for expected_id in [1_u64, 2] {
                let message = socket.next().await.unwrap().unwrap();
                let Message::Text(text) = message else {
                    panic!("expected inspector command");
                };
                let command: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
                assert_eq!(command["id"], expected_id);
                socket
                    .send(Message::Text(
                        serde_json::json!({"id": expected_id, "result": {}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            }

            let message = socket.next().await.unwrap().unwrap();
            let Message::Text(text) = message else {
                panic!("expected runIfWaitingForDebugger");
            };
            let command: serde_json::Value = serde_json::from_str(text.as_ref()).unwrap();
            assert_eq!(command["id"], 3);
            assert_eq!(command["method"], "Runtime.runIfWaitingForDebugger");
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "id": 3,
                        "error": { "code": -32000, "message": "not waiting" }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let expression = patch_expression(PatchOptions {
            disable_pet: true,
            disable_voice: false,
            fast_codex_startup: true,
        });
        let error = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            install_over_websocket(&format!("ws://{address}"), &expression, true),
        )
        .await
        .expect("protocol error should not wait for the outer startup timeout")
        .expect_err("runIfWaitingForDebugger error should fail installation");
        let message = error.to_string();
        assert!(
            message.contains("Runtime.runIfWaitingForDebugger"),
            "{message}"
        );
        assert!(message.contains("not waiting"), "{message}");
        server.await.unwrap();
    }
}
