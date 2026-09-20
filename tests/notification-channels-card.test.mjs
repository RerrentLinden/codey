import assert from "node:assert/strict";
import test from "node:test";

import { readSource } from "./helpers/read-source.mjs";
import {
  autoStubModule,
  collectElements,
  createModuleGraph,
  elementProps,
  elementType,
} from "./helpers/jsx-tree.mjs";

const [cardSource, policySource, stylesSource] = await Promise.all([
  readSource("src/notifications/NotificationChannelsCard.tsx"),
  readSource("src/FeaturePolicyCard.tsx"),
  readSource("src/styles.features.css"),
]);

const ui = autoStubModule("ui");
const icons = autoStubModule("icon");
const graph = createModuleGraph(
  new URL("../src/notifications/NotificationChannelsCard.tsx", import.meta.url),
  {
    autoStub: true,
    stubs: {
      "@tabler/icons-react": icons,
      "../components/ui": ui,
      "./FeishuChannelEditor": autoStubModule("feishu-editor"),
      "./NotificationChannelDialog": autoStubModule("channel-dialog"),
      "./NtfyChannelEditor": autoStubModule("ntfy-editor"),
      "./TelegramChannelEditor": autoStubModule("telegram-editor"),
      "./WechatClawChannelEditor": autoStubModule("wechat-claw-editor"),
      "./WecomChannelEditor": autoStubModule("wecom-editor"),
    },
  },
);

function channel(overrides = {}) {
  return {
    botToken: "",
    botTokenConfigured: false,
    chatId: "",
    contextToken: "",
    contextTokenConfigured: false,
    enabled: true,
    id: "channel-1",
    kind: "ntfy",
    url: "https://ntfy.example.com",
    urlConfigured: true,
    ...overrides,
  };
}

function renderCard(channels) {
  graph.reset();
  return graph.exports.NotificationChannelsCard({
    config: { webhook: { channels } },
    container: null,
    isBusy: false,
    onAddChannel: async () => true,
    onChannelChange: async () => true,
    onRequestRemoveChannel: () => {},
    popupContainer: null,
  });
}

const cardClasses = (tree) =>
  collectElements(
    tree,
    (element) =>
      typeof elementProps(element).className === "string" &&
      elementProps(element).className.split(" ").includes("notification-card"),
  ).map((element) => elementProps(element).className);

test("渠道列表保留 ul/li 语义与 aria-label", () => {
  const tree = renderCard([channel(), channel({ id: "channel-2", kind: "telegram" })]);
  const lists = collectElements(tree, (element) => elementType(element) === "ul");
  assert.equal(lists.length, 1, "渠道列表必须是真正的列表元素");
  assert.equal(elementProps(lists[0])["aria-label"], "已配置通知渠道");
  const items = collectElements(lists[0], (element) => elementType(element) === "li");
  assert.equal(items.length, 2, "每个渠道占一个列表项");
  assert.equal(cardClasses(tree).length, 2);
  assert.equal(elementProps(lists[0]).className, "notification-channel-list");

  const empty = renderCard([]);
  const emptyLists = collectElements(empty, (element) => elementType(element) === "ul");
  const emptyItems = collectElements(emptyLists[0], (element) => elementType(element) === "li");
  assert.equal(emptyItems.length, 1, "空态也必须放在列表项里");
  assert.match(elementProps(emptyItems[0]).className, /notification-empty-card/);
});

test("启用、停用与登录失效分别带上可区分的状态类", () => {
  const tree = renderCard([
    channel({ id: "enabled", kind: "ntfy" }),
    channel({ enabled: false, id: "disabled", kind: "ntfy" }),
    channel({ id: "expired", kind: "wechatClaw", sessionStatus: "expired" }),
  ]);
  assert.deepEqual(cardClasses(tree), [
    "notification-card codey-card active",
    "notification-card codey-card inactive",
    "notification-card codey-card expired",
  ]);
});

test("样式表定义三条状态规则与列表选择器，且不残留已废弃的网格类", () => {
  for (const state of ["active", "inactive", "expired"])
    assert.match(
      stylesSource,
      new RegExp(`\\.notification-card\\.${state}\\s*\\{`),
      state,
    );
  assert.match(stylesSource, /\.notification-channel-list\s*\{/);
  assert.match(stylesSource, /\.notification-channel-list > li\s*\{/);
  assert.match(stylesSource, /\.notification-channel-list > li > \.notification-card\s*\{/);
  assert.doesNotMatch(stylesSource, /\.notification-section-grid\s*\{/);
  assert.doesNotMatch(cardSource, /notification-section-grid/);
});

test("通知区域的 aria-labelledby 指向真实存在的标题", () => {
  const tree = renderCard([channel()]);
  assert.equal(
    collectElements(tree, (element) => elementProps(element).id === "notification-title")
      .length,
    1,
    "子组件必须渲染出被引用的标题",
  );
  assert.match(policySource, /aria-labelledby="notification-title"/);
  assert.doesNotMatch(policySource, /notification-section-title/);
});
