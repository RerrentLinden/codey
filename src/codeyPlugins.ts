export type PluginSchema = {
  type?: string; title?: string; description?: string; default?: unknown;
  enum?: unknown[]; properties?: Record<string, PluginSchema>; required?: string[];
  items?: PluginSchema; minimum?: number; maximum?: number; minLength?: number;
  maxLength?: number; minItems?: number; maxItems?: number; additionalProperties?: boolean;
};
export type CodeyPlugin = {
  id: string; name: string; version: string; description?: string; enabled: boolean;
  status: string; config: Record<string, unknown>; configSchema: PluginSchema;
  capabilities: string[]; lastError?: string; restartRequired?: boolean;
  activeVersion?: string | null; activeConfig?: Record<string, unknown> | null;
  pluginDir?: string; dataDir?: string; logDir?: string;
};
export type CodeyPluginsResult = { plugins: CodeyPlugin[]; platform: string; arch: string };
export type CodeyPluginPreview = {
  path: string; sha256: string; configSchema: PluginSchema;
  manifest: { id: string; name: string; version: string; description?: string;
    capabilities?: string[]; permissions?: string[]; headerNames?: string[] };
};

export function samePluginValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (!a || !b || typeof a !== "object" || typeof b !== "object") return false;
  if (Array.isArray(a) || Array.isArray(b)) return Array.isArray(a) && Array.isArray(b)
    && a.length === b.length && a.every((item, index) => samePluginValue(item, b[index]));
  const left = a as Record<string, unknown>, right = b as Record<string, unknown>;
  return Object.keys(left).length === Object.keys(right).length && Object.keys(left).every(key =>
    Object.prototype.hasOwnProperty.call(right, key) && samePluginValue(left[key], right[key]));
}

export function validatePluginConfig(value: unknown, schema: PluginSchema, path = "配置"): string[] {
  if (schema.enum && !schema.enum.some(item => samePluginValue(item, value))) return [`${path}：请选择允许的值`];
  const errors: string[] = [];
  if (schema.type === "object" || schema.properties) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return [`${path}：需要对象`];
    const object = value as Record<string, unknown>;
    for (const key of schema.required ?? []) if (!Object.prototype.hasOwnProperty.call(object, key) || object[key] === undefined) errors.push(`${path}.${key}：必填`);
    for (const [key, item] of Object.entries(object)) {
      const child = schema.properties && Object.prototype.hasOwnProperty.call(schema.properties, key) ? schema.properties[key] : undefined;
      if (child) errors.push(...validatePluginConfig(item, child, `${path}.${key}`));
      else if (schema.additionalProperties === false) errors.push(`${path}.${key}：不支持此字段`);
    }
  } else if (schema.type === "array") {
    if (!Array.isArray(value)) return [`${path}：需要数组`];
    if (schema.minItems != null && value.length < schema.minItems) errors.push(`${path}：至少 ${schema.minItems} 项`);
    if (schema.maxItems != null && value.length > schema.maxItems) errors.push(`${path}：最多 ${schema.maxItems} 项`);
    if (schema.items) value.forEach((item, index) => errors.push(...validatePluginConfig(item, schema.items!, `${path}[${index}]`)));
  } else if (schema.type === "boolean" && typeof value !== "boolean") errors.push(`${path}：需要开关值`);
  else if (schema.type === "string") {
    if (typeof value !== "string") return [`${path}：需要文本`];
    const length = Array.from(value).length;
    if (schema.minLength != null && length < schema.minLength) errors.push(`${path}：至少 ${schema.minLength} 个字符`);
    if (schema.maxLength != null && length > schema.maxLength) errors.push(`${path}：最多 ${schema.maxLength} 个字符`);
  } else if (schema.type === "number" || schema.type === "integer") {
    if (typeof value !== "number" || !Number.isFinite(value)) return [`${path}：需要有效数值`];
    if (schema.type === "integer" && !Number.isInteger(value)) errors.push(`${path}：需要整数`);
    if (schema.minimum != null && value < schema.minimum) errors.push(`${path}：不能小于 ${schema.minimum}`);
    if (schema.maximum != null && value > schema.maximum) errors.push(`${path}：不能大于 ${schema.maximum}`);
  } else if (schema.type === "null" && value !== null) errors.push(`${path}：需要 null`);
  return errors;
}

export function pluginStatusLabel(plugin: CodeyPlugin): string {
  if (plugin.lastError || plugin.status === "failed" || plugin.status === "error") return "运行异常";
  if (plugin.restartRequired) return "待重新启用";
  return plugin.enabled ? "已启用" : "已停用";
}

/** Fill explicit schema defaults without discarding undeclared stored keys. */
export function applyPluginDefaults(value: unknown, schema: PluginSchema): unknown {
  if (value === undefined && schema.default !== undefined) value = JSON.parse(JSON.stringify(schema.default));
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const result = { ...value as Record<string, unknown> };
    for (const [key, child] of Object.entries(schema.properties ?? {})) {
      const next = applyPluginDefaults(Object.prototype.hasOwnProperty.call(result, key) ? result[key] : undefined, child);
      if (next !== undefined) Object.defineProperty(result, key, { value: next, enumerable: true, writable: true, configurable: true });
    }
    return result;
  }
  if (Array.isArray(value) && schema.items) return value.map(item => applyPluginDefaults(item, schema.items!));
  return value;
}

export function initialPluginValue(schema: PluginSchema): unknown {
  if (schema.default !== undefined) return applyPluginDefaults(undefined, schema);
  if (schema.enum?.length) return JSON.parse(JSON.stringify(schema.enum[0]));
  if (schema.type === "object" || schema.properties) return applyPluginDefaults({}, schema);
  if (schema.type === "array") return [];
  if (schema.type === "boolean") return false;
  if (schema.type === "null") return null;
  return ""; // Blank numeric fields remain invalid until the user enters a number.
}

export function setPluginProperty(value: unknown, key: string, next: unknown): Record<string, unknown> {
  const result = value && typeof value === "object" && !Array.isArray(value) ? { ...value as Record<string, unknown> } : {};
  if (next === undefined) delete result[key];
  else Object.defineProperty(result, key, { value: next, enumerable: true, writable: true, configurable: true });
  return result;
}
