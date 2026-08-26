/** Validates custom API endpoints before they are saved. */
export function validateOutboundApiUrl(value: string, label = "API URL") {
  const normalized = value.trim();
  if (!normalized) return `请输入 ${label}`;
  try {
    const parsed = new URL(normalized);
    if (!["http:", "https:"].includes(parsed.protocol) || !parsed.hostname) {
      return `${label}必须是有效的 HTTP(S) 地址`;
    }
    if (parsed.username || parsed.password) {
      return `${label}不能包含用户名或密码，请在 Key 字段单独填写凭据`;
    }
    return "";
  } catch {
    return `${label}格式无效`;
  }
}
