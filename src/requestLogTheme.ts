/** 独立浏览器页面沿用打开时的 Codex 主题，旧链接跟随系统外观。 */
export function installRequestLogTheme(ownerDocument = document) {
  const view = ownerDocument.defaultView!;
  const requested = new URLSearchParams(view.location.search).get("theme");
  const theme = requested === "light" || requested === "dark" ? requested : null;
  const preference = view.matchMedia("(prefers-color-scheme: dark)");
  const sync = () => {
    const resolved = theme ?? (preference.matches ? "dark" : "light");
    ownerDocument.documentElement.dataset.theme = resolved;
    ownerDocument.documentElement.style.colorScheme = resolved;
  };
  sync();
  if (!theme) preference.addEventListener("change", sync);
  return () => preference.removeEventListener("change", sync);
}
