import { validatePluginConfigText } from "./codeyPlugins";

export type PluginConfigEntry = {
  id: string; key: string; path: (string | number)[];
  kind: "object" | "array" | "string" | "number" | "boolean" | "null";
  comment?: string; valueText: string; children: PluginConfigEntry[];
};
export type PluginConfigDocument = { content: string; entries: PluginConfigEntry[] };

type TokenNode = { key: string | number; path: (string | number)[]; start: number; end: number;
  kind: PluginConfigEntry["kind"]; valueText: string; children: TokenNode[] };
type CommentScope = { node: TokenNode; descriptions: Map<string, string>; keys: Set<string> };
const spans = new WeakMap<PluginConfigDocument, Map<string, TokenNode>>();
const numberPattern = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$/;
const maxDepth = 128;

export function parsePluginConfigDocument(content: string): PluginConfigDocument {
  const error = validatePluginConfigText(content);
  if (error) throw new Error(error);
  let offset = 0;
  const whitespace = () => { while (/[\t\n\r ]/.test(content[offset] ?? "x")) offset++; };
  const stringToken = (): string => {
    const start = offset++;
    while (offset < content.length) {
      const char = content[offset++];
      if (char === "\\") offset++;
      else if (char === '"') return JSON.parse(content.slice(start, offset)) as string;
    }
    throw new Error("JSON 字符串不完整。");
  };
  const read = (key: string | number, path: (string | number)[], depth: number): TokenNode => {
    if (depth > maxDepth) throw new Error(`配置嵌套不能超过 ${maxDepth} 层。`);
    whitespace();
    const start = offset;
    const children: TokenNode[] = [];
    let kind: PluginConfigEntry["kind"];
    let valueText = "";
    const first = content[offset];
    if (first === "{" || first === "[") {
      kind = first === "{" ? "object" : "array";
      const close = first === "{" ? "}" : "]";
      const keys = new Set<string>();
      offset++;
      whitespace();
      while (content[offset] !== close) {
        let childKey: string | number = children.length;
        if (kind === "object") {
          childKey = stringToken();
          if (keys.has(childKey)) throw new Error(`配置字段 ${JSON.stringify([...path, childKey])} 重复，无法安全编辑。`);
          keys.add(childKey);
          whitespace();
          offset++;
        }
        children.push(read(childKey, [...path, childKey], depth + 1));
        whitespace();
        if (content[offset] !== ",") break;
        offset++;
        whitespace();
      }
      offset++;
    } else if (first === '"') {
      kind = "string";
      valueText = stringToken();
    } else {
      while (offset < content.length && !/[\t\n\r ,}\]]/.test(content[offset])) offset++;
      valueText = content.slice(start, offset);
      kind = valueText === "null" ? "null" : valueText === "true" || valueText === "false" ? "boolean" : "number";
      if (kind === "number" && !Number.isFinite(Number(valueText))) throw new Error(`配置字段 ${JSON.stringify(path)} 的数字必须为有限值。`);
    }
    return { key, path, start, end: offset, kind, valueText, children };
  };
  const root = read("", [], 0);
  const index = new Map<string, TokenNode>();
  const scopeFor = (node: TokenNode): CommentScope => ({ node,
    descriptions: new Map(node.children.find(child => child.key === "_comments")?.children.map(child => [String(child.key), child.valueText]) ?? []),
    keys: new Set(node.children.map(child => String(child.key))),
  });
  const entries = (node: TokenNode, scopes: CommentScope[]): PluginConfigEntry[] => {
    const nextScopes = node.kind === "object" ? [...scopes, scopeFor(node)] : scopes;
    return node.children.filter(child => node.kind !== "object" || child.key !== "_comments").map(child => {
      const id = JSON.stringify(child.path);
      let comment: string | undefined;
      if (typeof child.key === "string") {
        for (let i = nextScopes.length - 1; i >= 0; i--) {
          const scope = nextScopes[i];
          const relative = child.path.slice(scope.node.path.length).filter(part => typeof part === "string");
          const name = relative.join(".");
          if (relative.length > 1 && scope.keys.has(name)) continue;
          if (scope.descriptions.has(name)) { comment = scope.descriptions.get(name); break; }
        }
      }
      index.set(id, child);
      return { id, key: typeof child.key === "number" ? `[${child.key}]` : child.key,
        path: [...child.path], kind: child.kind, comment, valueText: child.valueText, children: entries(child, nextScopes) };
    });
  };
  const document = { content, entries: entries(root, []) };
  spans.set(document, index);
  return document;
}

export function validatePluginConfigValue(entry: PluginConfigEntry, text: string): string | undefined {
  switch (entry.kind) {
    case "object": case "array": return "只能修改配置项的值，不能修改配置结构。";
    case "string": return undefined;
    case "number": return numberPattern.test(text) && Number.isFinite(Number(text)) ? undefined : "请输入有效的有限数字。";
    case "boolean": return text === "true" || text === "false" ? undefined : "布尔值只能为 true 或 false。";
    case "null": {
      try {
        const value: unknown = JSON.parse(text);
        if (value !== null && typeof value === "object") return "此配置项只能改为字符串、数字、布尔值或 null。";
        if (typeof value === "number" && !Number.isFinite(value)) return "请输入有效的有限数字。";
        return undefined;
      } catch { return "请输入有效的 JSON 标量值；字符串需使用双引号。"; }
    }
  }
}

export function serializePluginConfigDocument(document: PluginConfigDocument, edits: ReadonlyMap<string, string>): string {
  // 从原文重新获取位置及类型，避免界面对象被修改后影响允许编辑的范围。
  const original = parsePluginConfigDocument(document.content);
  const index = spans.get(original)!;
  const replacements: { start: number; end: number; text: string }[] = [];
  for (const [id, text] of edits) {
    const node = index.get(id);
    if (!node) throw new Error("配置项不存在，无法保存修改。");
    const entry: PluginConfigEntry = { id, key: String(node.key), path: node.path, kind: node.kind, valueText: node.valueText, children: [] };
    const error = validatePluginConfigValue(entry, text);
    if (error) throw new Error(`${JSON.stringify(node.path)}：${error}`);
    if (text === node.valueText) continue;
    replacements.push({ start: node.start, end: node.end, text: node.kind === "string" ? JSON.stringify(text) : text });
  }
  let content = document.content;
  for (const replacement of replacements.sort((left, right) => right.start - left.start)) {
    content = content.slice(0, replacement.start) + replacement.text + content.slice(replacement.end);
  }
  const error = validatePluginConfigText(content);
  if (error) throw new Error(error);
  return content;
}
