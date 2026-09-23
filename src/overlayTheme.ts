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
  // 主题只存在于文档根、body 和 #root。Codex 会频繁增删这些节点的直接子元素，
  // 子节点变化本身不改变主题；只有根集合换成新节点时才需要重新计算。
  let observedRoots: HTMLElement[] = [];
  const observeRoots = () => {
    const next = themeRoots(ownerDocument);
    if (
      next.length === observedRoots.length &&
      next.every((element, index) => element === observedRoots[index])
    ) {
      return false;
    }
    observer.disconnect();
    observedRoots = next;
    for (const element of next) {
      observer.observe(element, {
        attributes: true,
        attributeFilter: ["data-theme", "class", "style"],
        childList: true,
      });
    }
    return true;
  };
  const observer = new view.MutationObserver((records) => {
    const structureChanged = records.some((record) => record.type === "childList");
    const attributesChanged = records.some((record) => record.type === "attributes");
    if (!structureChanged) {
      if (attributesChanged) sync();
      return;
    }
    const rootsChanged = observeRoots();
    if (rootsChanged || attributesChanged) sync();
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
