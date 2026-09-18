type Theme = "light" | "dark";

function explicitTheme(element: Element): Theme | undefined {
  const theme = element.getAttribute("data-theme");
  if (theme === "light" || theme === "dark") return theme;
  if (element.classList.contains("dark")) return "dark";
  if (element.classList.contains("light")) return "light";
  return undefined;
}

function themeRoots(ownerDocument: Document) {
  return [...new Set([
    ownerDocument.documentElement,
    ownerDocument.body,
    ownerDocument.getElementById("root"),
  ].filter((element): element is HTMLElement => element !== null))];
}

/** 从宿主已应用的主题读取外观，不依赖 Codex 私有配置存储。 */
export function readHostTheme(ownerDocument = document): Theme {
  const view = ownerDocument.defaultView!;
  const elements = themeRoots(ownerDocument);
  for (const element of elements) {
    const theme = explicitTheme(element);
    if (theme) return theme;
  }
  for (const element of elements) {
    const scheme = view.getComputedStyle(element).colorScheme.split(/\s+/);
    const light = scheme.includes("light");
    const dark = scheme.includes("dark");
    if (light !== dark) return dark ? "dark" : "light";
  }
  return view.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function installOverlayTheme(containers: HTMLElement[], ownerDocument = document) {
  const view = ownerDocument.defaultView!;
  const preference = view.matchMedia("(prefers-color-scheme: dark)");
  const sync = () => {
    const theme = readHostTheme(ownerDocument);
    for (const container of containers) {
      if (container.dataset.theme !== theme) container.dataset.theme = theme;
      container.style.colorScheme = theme;
    }
  };
  const observeRoots = () => {
    observer.disconnect();
    for (const element of themeRoots(ownerDocument)) {
      observer.observe(element, {
        attributes: true,
        attributeFilter: ["data-theme", "class", "style"],
        // 只观察根节点，避免会话正文频繁更新触发主题计算。
        childList: true,
      });
    }
  };
  const observer = new view.MutationObserver((records) => {
    if (records.some((record) => record.type === "childList")) observeRoots();
    sync();
  });
  observeRoots();
  preference.addEventListener("change", sync);
  // 首次渲染前同步，隐藏期间保留监听，避免重新打开时短暂出现旧主题。
  sync();

  return {
    sync,
    dispose() {
      observer.disconnect();
      preference.removeEventListener("change", sync);
    },
  };
}
