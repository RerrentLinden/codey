// 页面脱敏：只用于界面展示，不改变保存的配置内容。
const MASK = "\u2022";

function maskMiddle(value: string, keepStart: number, keepEnd: number) {
  const characters = Array.from(value);
  if (characters.length <= keepStart + keepEnd) {
    const head = characters.length > 2 ? 1 : 0;
    return characters.slice(0, head).join("") + MASK.repeat(characters.length - head);
  }
  const head = characters.slice(0, keepStart).join("");
  const tail = keepEnd > 0 ? characters.slice(characters.length - keepEnd).join("") : "";
  return head + MASK.repeat(characters.length - keepStart - keepEnd) + tail;
}

function maskHost(host: string) {
  const labels = host.split(".");
  if (labels.length === 4 && labels.every((label) => /^\d+$/.test(label))) {
    return `${labels[0]}.${MASK.repeat(3)}.${MASK.repeat(3)}.${labels[3]}`;
  }
  if (labels.length < 2) return maskMiddle(host, 3, 0);
  const tail = labels[labels.length - 1];
  return `${maskMiddle(labels.slice(0, -1).join("."), 3, 0)}.${tail}`;
}

export function maskUrl(value: string) {
  const trimmed = value.trim();
  if (!trimmed) return value;
  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return maskMiddle(trimmed, 4, 0);
  }
  const path = `${parsed.pathname}${parsed.search}${parsed.hash}`;
  const suffix = path === "/" ? "" : path;
  return `${parsed.protocol}//${maskHost(parsed.hostname)}${parsed.port ? `:${parsed.port}` : ""}${suffix}`;
}

export function maskEmail(value: string) {
  const trimmed = value.trim();
  const at = trimmed.lastIndexOf("@");
  if (at <= 0 || at === trimmed.length - 1) return maskMiddle(trimmed, 2, 0);
  const local = trimmed.slice(0, at);
  return `${maskMiddle(local, local.length >= 4 ? 2 : 1, 0)}@${trimmed.slice(at + 1)}`;
}
