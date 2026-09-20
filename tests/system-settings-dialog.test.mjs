import assert from "node:assert/strict";
import test from "node:test";

import { readSource } from "./helpers/read-source.mjs";

const [dialogSource, appSource, stylesSource, operationsSource] = await Promise.all([
  readSource("src/SystemSettingsDialog.tsx"),
  readSource("src/App.tsx"),
  readSource("src/styles.css"),
  readSource("src/OperationsPanel.tsx"),
]);

test("SystemSettingsDialog renders versions, Codex path, repair button, and auto-update switch", () => {
  assert.match(dialogSource, /export const SystemSettingsDialog = memo\(/);
  // Codey 版本
  assert.match(dialogSource, /Codey 版本/);
  assert.match(dialogSource, /appVersion \|\| "0\.0\.1"/);
  // Codex 版本
  assert.match(dialogSource, /Codex 版本/);
  assert.match(dialogSource, /codexAppVersion/);
  // 不展示冗余的应用及运行状态徽章
  assert.doesNotMatch(dialogSource, /当前应用/);
  assert.doesNotMatch(dialogSource, />运行中</);
  // Codex 目录与复制按钮
  assert.match(dialogSource, /Codex 目录/);
  assert.match(dialogSource, /handleCopyPath/);
  // 修复 Codex 配置按钮与 notice
  assert.match(dialogSource, /修复 Codex 配置/);
  assert.match(dialogSource, /onRepairCodexConfig/);
  assert.match(dialogSource, /:\s*"修复"\}/);
  assert.match(dialogSource, /<IconTool/);
  assert.doesNotMatch(dialogSource, /检查配置文件并修复异常/);
  assert.match(dialogSource, /configRepairNotice/);
  // 自动检查 Codey 更新开关
  assert.match(dialogSource, /自动检查 Codey 更新/);
  assert.doesNotMatch(dialogSource, /后台每隔 30 分钟自动检查新版本/);
  assert.match(dialogSource, /autoCheckCodeyUpdates/);
  assert.match(dialogSource, /onAutoCheckCodeyUpdatesChange/);
});

test("App sidebar footer renders left version and update icon, right settings icon", () => {
  assert.match(appSource, /className="sidebar-footer-bar"/);
  assert.match(appSource, /className="sidebar-footer-left"/);
  assert.doesNotMatch(appSource, /className="sidebar-footer-brand"/);
  assert.match(appSource, /className="sidebar-footer-version font-mono"/);
  assert.match(appSource, /<IconCircleArrowUp size=\{15\}/);
  assert.match(appSource, /handleFooterUpdateClick/);
  assert.match(appSource, /className="sidebar-footer-right"/);
  assert.match(appSource, /setSystemSettingsOpen\(true\)/);
  assert.match(appSource, /<SystemSettingsDialog[\s\S]*open=\{systemSettingsOpen\}/);
});

test("styles define sidebar footer layout and responsive behavior", () => {
  assert.match(stylesSource, /\.sidebar-footer-bar\s*\{/);
  assert.match(stylesSource, /\.sidebar-footer-left\s*\{/);
  assert.match(stylesSource, /\.sidebar-footer-right\s*\{/);
  assert.match(stylesSource, /\.sidebar-footer-version\s*\{/);
});

test("OperationsPanel removes redundant top hero card and config repair button", () => {
  assert.doesNotMatch(operationsSource, /operations-hero-card/);
  assert.doesNotMatch(operationsSource, /Codex 运行状态/);
  assert.doesNotMatch(operationsSource, /修复 Codex 配置/);
  assert.doesNotMatch(operationsSource, /expanded-tray-toolbar/);
  assert.match(operationsSource, /已生效 \$\{enabledOptimizationFeatures\.length\} 项功能/);
});
