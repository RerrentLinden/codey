import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const panel = await readFile(new URL("../src/OfficialAccountsPanel.tsx", import.meta.url), "utf8");
const styles = await readFile(new URL("../src/styles.models.css", import.meta.url), "utf8");
const types = await readFile(new URL("../src/App.types.ts", import.meta.url), "utf8");
const mockApi = await readFile(new URL("../src/dev/mockApi.ts", import.meta.url), "utf8");
const store = await readFile(new URL("../backend/src/official_accounts.rs", import.meta.url), "utf8");
const commands = await readFile(new URL("../backend/src/commands.rs", import.meta.url), "utf8");
const provider = await readFile(new URL("../backend/src/codex_provider.rs", import.meta.url), "utf8");
const accountCommands = await readFile(
  new URL("../backend/src/commands/official_accounts.rs", import.meta.url),
  "utf8",
);

test("失效账号卡片使用红色背景并显示失效标识", () => {
  assert.match(types, /invalid\?: boolean;/);
  assert.match(types, /invalidReason\?: string;/);
  assert.match(styles, /\.official-account-item\.is-invalid \{[\s\S]*?background: rgba\(255, 59, 48/);
  assert.match(panel, /account\.invalid \? " is-invalid" : ""/);
  assert.match(panel, /<Badge variant="destructive" title=\{account\.invalidReason\}>/);
  assert.match(panel, /账号已失效/);
});

test("失效账号隐藏切换默认按钮，后端也拒绝切换", () => {
  assert.match(panel, /\{!account\.isDefault && !account\.invalid && \(/);
  const setDefault = accountCommands.slice(
    accountCommands.indexOf("pub(super) async fn set_default_official_account"),
  );
  assert.match(setDefault, /if record\.invalid\(\) \{[\s\S]{0,240}无法设为默认/);
});

test("额度查询确认失效后重读列表，且不再请求官方接口", () => {
  assert.match(panel, /snapshot\.reason === "official_account_invalid"/);
  assert.match(commands, /record\.invalid_reason\(\)[\s\S]{0,240}official_account_invalid/);
  // 只有 OAuth 明确的 invalid_grant 才标记失效，其他错误只记日志。
  assert.match(store, /official_account_invalid_from_body/);
  assert.match(store, /invalid_grant/);
});

test("失效账号不再重复请求官方接口", () => {
  // 令牌刷新前直接返回本地记录，不再向官方令牌接口发请求。
  assert.match(
    accountCommands,
    /if record\.invalid\(\) \{\s*return Ok\(record\);\s*\}\s*match refresh_if_stale/,
  );
  assert.match(
    store,
    /pub async fn refresh_if_stale\([\s\S]{0,400}if record\.invalid\(\) \{\s*return Ok\(false\);\s*\}/,
  );
  // 前端轮询跳过已标记失效的账号，只在失效状态变化时重算线路。
  assert.match(panel, /if \(account\.invalid\) \{[\s\S]{0,200}invalidUsageSnapshot\(account\)/);
  assert.match(panel, /snapshot\.reason === "official_account_invalid" && !known\?\.invalid/);
  assert.match(panel, /const recovered = Boolean\(known\?\.invalid\) && snapshot\.status === "ok"/);
  assert.match(panel, /if \(becameInvalid \|\| recovered\) \{/);
  assert.match(panel, /account\.invalid \? 1 : 0/);
});

test("预览数据包含失效账号，便于界面检查", () => {
  assert.match(mockApi, /invalid: true, invalidReason:/);
  assert.match(mockApi, /reason: "official_account_invalid"/);
  assert.match(mockApi, /status: "failed", message: "该账号已失效，无法设为默认；请重新添加账号"/);
});

test("失效账号的线路随失效标记一起移除", () => {
  // 派生线路时跳过失效账号，编号仍按全部账号的添加顺序计算。
  assert.match(provider, /records\.retain\(\|record\| !record\.invalid\(\)\)/);
  assert.match(provider, /has_stored_accounts/);
  // 账号还在但凭据全部失效时清掉线路，不再报成需要重新登录。
  assert.match(commands, /stored_accounts_invalid/);
  // 标记失效后立刻重算线路，并且前端能取回最新的配置与模型状态。
  assert.match(commands, /refresh_official_routes_after_invalid_account/);
  assert.match(commands, /"refresh_official_account_routes" =>/);
  assert.match(
    accountCommands,
    /pub\(super\) async fn refresh_official_route_after_account_change/,
  );
  assert.match(
    panel,
    /invoke<OfficialAccountsResult>\("refresh_official_account_routes"\)/,
  );
  // 预览派生同样跳过失效账号，界面检查时线路列表与后端一致。
  assert.match(mockApi, /\.filter\(\(account\) => !account\.invalid\)/);
});
