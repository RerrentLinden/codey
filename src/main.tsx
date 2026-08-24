import React from "react";
import ReactDOM from "react-dom/client";
import { MantineProvider } from "@mantine/core";
import "@mantine/core/styles.css";
import { App } from "./App";
import type { CcSwitchStatus, Config, ModelState, Profile } from "./App.types";
import {
  includesModelId,
  modelIdsEqual,
  modelKey,
  uniqueModelIds,
} from "./modelIds";
import { previewOfficialModels, previewUpstreamModels } from "./previewModels";
import { codeyMantineTheme } from "./mantine";
import "./tailwind.css";
import {
  previewCrashpadPendingStats,
  previewTraceLogStats,
} from "./previewTraceLogStats";
import "./styles.css";
import "./styles.operations.css";
import "./styles.models.css";
import "./styles.features.css";
import "./styles.diagnostics.css";
import "./styles.responsive.css";

// 在 Vite 开发模式下，若未通过 Codey Bridge/Token 访问，自动注入 Mock 接口方便 UI 调试
if (import.meta.env.DEV) {
  if (!window.__codeyInvokeApi) {
    console.log("[Dev Mode] Auto-injecting Codey Mock API");
    const previewClientPlatform =
      new URLSearchParams(window.location.search).get("platform") === "windows"
        ? "windows"
        : "macos";
    const previewEndpoints = {
      primary: "https://primary.example.invalid/v1",
      backup: "https://backup.example.invalid/v1",
      feishu: "https://webhook.example.invalid/feishu/preview-only",
      wecom: "https://webhook.example.invalid/wecom/preview-only?key=preview",
    } as const;
    const previewApiKey = "preview-only-not-a-secret";
    let previewConfig: Config = {
      settingsRevision: 0,
      activeProfileId: "primary",
      initialRouteImportCompleted: true,
      profiles: [
        {
          id: "primary",
          name: "主力代理 (ChatGPT)",
          baseUrl: previewEndpoints.primary,
          apiKey: "",
          upstreamProtocol: "openaiResponses",
          authMode: "apiKey",
          apiKeyConfigured: true,
          clearApiKey: false,
          ccSwitchProviderId: "primary",
          ccSwitchReadOnly: false,
          supportsRemoteCompaction: false,
        },
        {
          id: "backup",
          name: "备用中转 (Claude)",
          baseUrl: previewEndpoints.backup,
          apiKey: "",
          upstreamProtocol: "openaiCompatible",
          authMode: "apiKey",
          apiKeyConfigured: true,
          clearApiKey: false,
          ccSwitchProviderId: "backup",
          ccSwitchReadOnly: false,
          supportsRemoteCompaction: false,
        },
      ],
      webhook: {
        channels: [
          {
            id: "preview-feishu",
            kind: "feishu" as const,
            enabled: true,
            url: previewEndpoints.feishu,
            urlConfigured: true,
            clearUrl: false,
            botToken: "",
            botTokenConfigured: false,
            clearBotToken: false,
            chatId: "",
          },
          {
            id: "preview-wecom",
            kind: "wecom" as const,
            enabled: true,
            url: previewEndpoints.wecom,
            urlConfigured: true,
            clearUrl: false,
            botToken: "",
            botTokenConfigured: false,
            clearBotToken: false,
            chatId: "",
          },
          {
            id: "preview-telegram",
            kind: "telegram" as const,
            enabled: false,
            url: "",
            urlConfigured: false,
            clearUrl: false,
            botToken: "",
            botTokenConfigured: true,
            clearBotToken: false,
            chatId: "preview-chat-id",
          },
        ],
      },
      promptOptimization: {
        enabled: true,
        baseUrl: previewEndpoints.primary,
        apiKey: "",
        apiKeyConfigured: true,
        clearApiKey: false,
        model: "gpt-5.6-sol",
        instruction: "",
      },
      codexAppPath: "/Applications/ChatGPT.app",
      userScripts: [],
      selectedModelsByProvider: {
        primary: ["provider-fast-coder", "claude-sonnet-4-5"],
      },
      manualThirdPartyModelsByProvider: {
        primary: ["provider-fast-coder"],
      },
      declaredOfficialModelsByProvider: {} as Record<string, string[]>,
      upstreamModelsByProvider: { primary: previewUpstreamModels },
      defaultModelByProvider: {},
      disableTraceLogWrites: true,
      protectCrashpadPending: true,
      slimCodexPet: true,
      gpuLaunchMode: "off" as const,
      fastContextTools: false,
      subagentOptimization: false,
      subagentModel: "gpt-5.6-terra",
      subagentReasoningEffort: "medium",
      subagentRoles: {
        codey_quick_scan: { model: "gpt-5.6-sol", reasoningEffort: "low" },
        codey_deep_research: { model: "gpt-5.6-sol", reasoningEffort: "high" },
        codey_visual_analysis: { model: "gpt-5.6-sol", reasoningEffort: "high" },
        codey_worker: { model: "provider-fast-coder", reasoningEffort: "medium" },
        codey_visual_worker: { model: "gpt-5.6-sol", reasoningEffort: "high" },
        default: { model: "gpt-5.6-sol", reasoningEffort: "medium" },
      },
      hideFullAccessWarning: false,
      showAccountUsageInHeader: true,
    };
    let previewModelState: ModelState = {
      officialModels: previewOfficialModels.map((model) => ({
        ...model,
        supported: includesModelId(previewUpstreamModels, model.slug),
      })),
      officialModelIds: previewOfficialModels.map((model) => model.slug),
      thirdPartyModels: ["provider-fast-coder", "claude-sonnet-4-5"],
      manualThirdPartyModels: ["provider-fast-coder"],
      upstreamModels: previewUpstreamModels,
      defaultModel: "gpt-5.6-sol",
    };
    const routeProviderId = (profile: Profile) =>
      profile.ccSwitchProviderId || profile.id;
    const activePreviewProfile = () =>
      previewConfig.profiles.find(
        (profile) => profile.id === previewConfig.activeProfileId,
      ) || previewConfig.profiles[0];
    const previewCcSwitch = (): CcSwitchStatus => {
      const profile = activePreviewProfile();
      return {
        changed: false,
        provider: {
          id: profile ? routeProviderId(profile) : "openai",
          name: profile?.name || "OpenAI 官方直登",
          official: profile?.authMode === "officialAccount",
          baseUrl: profile?.baseUrl || "",
          localRoute: false,
        },
      };
    };
    const previewModelStateForProfile = (profile: Profile): ModelState => {
      const providerId = routeProviderId(profile);
      const official = profile.authMode === "officialAccount";
      const upstream = previewConfig.upstreamModelsByProvider[providerId] || [];
      const selected = previewConfig.selectedModelsByProvider[providerId] || [];
      const manual = previewConfig.manualThirdPartyModelsByProvider[providerId] || [];
      const selectableOfficial = previewOfficialModels.filter(
        (model) => official || includesModelId(upstream, model.slug),
      );
      const thirdPartyModels = official ? [] : selected;
      const requestedDefault = previewConfig.defaultModelByProvider[providerId];
      const defaultModel =
        [
          ...selectableOfficial.map((model) => model.slug),
          ...thirdPartyModels,
        ].find(
          (model) =>
            Boolean(requestedDefault) && modelIdsEqual(model, requestedDefault),
        ) ||
        selectableOfficial[0]?.slug ||
        thirdPartyModels[0] ||
        "";
      return {
        officialModels: previewOfficialModels.map((model) => ({
          ...model,
          supported: official || includesModelId(upstream, model.slug),
        })),
        officialModelIds: previewOfficialModels.map((model) => model.slug),
        thirdPartyModels,
        manualThirdPartyModels: manual.filter((model) =>
          includesModelId(thirdPartyModels, model),
        ),
        upstreamModels: official ? [] : upstream,
        defaultModel,
      };
    };
    const refreshPreviewModelState = () => {
      const profile = activePreviewProfile();
      if (profile) previewModelState = previewModelStateForProfile(profile);
    };
    let previewTraceStats: typeof previewTraceLogStats | undefined;
    let previewCrashpadStats:
      | typeof previewCrashpadPendingStats
      | undefined = previewCrashpadPendingStats;

    window.__codeyInvokeApi = async (command, args) => {
      console.log(`[Mock API Call] ${command}`, args);
      // Wait a tiny bit to simulate network delay
      await new Promise((resolve) => setTimeout(resolve, 300));

      if (command === "load_codey_config") {
        return {
          config: previewConfig,
          modelState: previewModelState,
          startupError: undefined,
          ccSwitch: previewCcSwitch(),
          fastContextToolsStatus: {
            userConfigured: false,
            detectionFailed: false,
          },
        };
      }
      if (command === "runtime_status") {
        const activeNotificationChannelCount =
          previewConfig.webhook.channels.filter(
            (channel) =>
              channel.enabled &&
              (channel.kind === "telegram"
                ? channel.botTokenConfigured && Boolean(channel.chatId.trim())
                : channel.urlConfigured),
          ).length;
        return {
          running: true,
          appVersion: "0.2.0",
          codexAppVersion: "26.601.21317",
          clientPlatform: previewClientPlatform,
          restartRequired: false,
          restartInProgress: false,
          activeProfileId: previewConfig.activeProfileId,
          activeProfileName:
            previewConfig.profiles.find(
              (p) => p.id === previewConfig.activeProfileId,
            )?.name || "未命名代理",
          codexAppPath: previewConfig.codexAppPath,
          maintenance: {
            sessionStatus: "ready",
            sessionFilesFixed: 3,
            sqliteRowsUpdated: 7,
            ghostTasksPruned: 2,
            performanceStatus: "ready",
            performanceDetail:
              previewClientPlatform === "windows"
                ? "Windows 启动补丁已安装：WMI 周期采样保护等待运行时确认，临时 WebView 与执行环境回收已启用"
                : "启动补丁已启用：临时 WebView 和执行环境会自动回收",
          },
          injectionScripts: [
            {
              id: "bridge-helpers",
              name: "桥接辅助",
              source: "builtin",
              visibility: "internal",
              status: "effective",
              detail: "桥接函数可调用",
            },
            ...(previewClientPlatform === "windows"
              ? [
                  {
                    id: "windows-wmi-sampler",
                    name: "Windows WMI 周期采样保护",
                    source: "builtin" as const,
                    visibility: "feature" as const,
                    status: "effective" as const,
                    detail: "已阻止 2 次 WMI 周期进程采样",
                  },
                ]
              : []),
            {
              id: "model-whitelist",
              name: "模型白名单",
              source: "builtin",
              visibility: "internal",
              status: "effective",
              detail: "模型目录已加载（5 个模型）",
            },
            {
              id: "pet-control-shield",
              name: "宠物控制精简",
              source: "builtin",
              visibility: "feature",
              status: "effective",
              detail: "宠物控制精简已启用",
            },
            {
              id: "security-warning-shield",
              name: "安全提示控制",
              source: "builtin",
              visibility: "feature",
              status: "inactive",
              detail: "控制器已就绪，当前屏蔽策略关闭",
            },
            {
              id: "settings-overlay-loader",
              name: "配置面板加载器",
              source: "builtin",
              visibility: "internal",
              status: "effective",
              detail: "配置面板按需加载器可用",
            },
            {
              id: "renderer-controls",
              name: "渲染器控制",
              source: "builtin",
              visibility: "internal",
              status: "effective",
              detail: "渲染器控制与按需加载 API 可用",
            },
            {
              id: "plugin-marketplace-compatibility",
              name: "插件市场兼容",
              source: "builtin",
              visibility: "internal",
              status: "effective",
              detail: "插件市场桥接已接管",
            },
          ],
          fastContextToolsActive: previewConfig.fastContextTools,
          subagentOptimizationActive: previewConfig.subagentOptimization,
          notificationChannelsActive: activeNotificationChannelCount > 0,
          activeNotificationChannelCount,
          traceLogWriteProtectionActive: previewConfig.disableTraceLogWrites,
          crashpadDiskProtectionActive:
            previewClientPlatform === "macos" &&
            previewConfig.protectCrashpadPending,
          ...(previewTraceStats ? { traceLogStats: previewTraceStats } : {}),
          ...(previewCrashpadStats
            ? { crashpadPendingStats: previewCrashpadStats }
            : {}),
        };
      }
      if (command === "refresh_diagnostic_storage_stats") {
        previewTraceStats = previewTraceLogStats;
        previewCrashpadStats = {
          ...previewCrashpadPendingStats,
          protectionEnabled: previewConfig.protectCrashpadPending,
        };
        return {
          status: "ok",
          traceLogStats: previewTraceStats,
          crashpadPendingStats: previewCrashpadStats,
        };
      }
      if (command === "refresh_trace_log_stats") {
        previewTraceStats = previewTraceLogStats;
        return { status: "ok", traceLogStats: previewTraceStats };
      }
      if (command === "save_codey_config") {
        const incoming = args.config as Config;
        previewConfig = {
          ...incoming,
          profiles: incoming.profiles.map((profile) => ({
            ...profile,
            apiKey: "",
            apiKeyConfigured: profile.clearApiKey
              ? false
              : Boolean(profile.apiKey.trim() || profile.apiKeyConfigured),
            clearApiKey: false,
          })),
          settingsRevision: previewConfig.settingsRevision + 1,
        };
        refreshPreviewModelState();
        return {
          config: previewConfig,
          modelState: previewModelState,
          ccSwitch: previewCcSwitch(),
          fastContextToolsStatus: {
            userConfigured: false,
            detectionFailed: false,
          },
          restartRequired: false,
        };
      }
      if (command === "reveal_notification_channel") {
        const channelId = String(args.channelId || "");
        const channel = previewConfig.webhook.channels.find(
          (candidate) => candidate.id === channelId,
        );
        return channel
          ? { channel }
          : { status: "failed", message: "找不到要编辑的通知渠道" };
      }
      if (command === "reveal_prompt_optimization_api_key") {
        return previewConfig.promptOptimization.apiKeyConfigured
          ? { apiKey: previewApiKey }
          : {
              status: "failed",
              message: "提示词优化 API Key 尚未保存",
            };
      }
      if (command === "sync_current_provider") {
        return {
          config: previewConfig,
          modelState: previewModelState,
          ccSwitch: previewCcSwitch(),
          restartRequired: false,
        };
      }
      if (
        command === "save_route" ||
        command === "activate_route" ||
        command === "delete_route" ||
        command === "fetch_route_models"
      ) {
        const expectedRevision = Number(args.expectedRevision);
        if (expectedRevision !== previewConfig.settingsRevision) {
          return {
            status: "failed",
            message: "Codey 设置已被其他操作更新，请重新载入后再操作线路",
          };
        }
      }
      if (command === "save_route") {
        const route = args.route as Profile;
        const existing = previewConfig.profiles.find(
          (profile) => profile.id === route.id,
        );
        const savedRoute: Profile = {
          ...route,
          apiKey: "",
          apiKeyConfigured: route.clearApiKey
            ? false
            : Boolean(route.apiKey.trim() || route.apiKeyConfigured || existing?.apiKeyConfigured),
          clearApiKey: false,
        };
        previewConfig = {
          ...previewConfig,
          settingsRevision: previewConfig.settingsRevision + 1,
          activeProfileId: savedRoute.id,
          profiles: existing
            ? previewConfig.profiles.map((profile) =>
                profile.id === savedRoute.id ? savedRoute : profile,
              )
            : [...previewConfig.profiles, savedRoute],
        };
        refreshPreviewModelState();
        return {
          status: "ok",
          config: previewConfig,
          modelState: previewModelState,
          ccSwitch: previewCcSwitch(),
          restartRequired: !existing,
          modelHotReloaded: Boolean(existing),
        };
      }
      if (command === "activate_route") {
        const routeId = String(args.routeId || "");
        if (!previewConfig.profiles.some((profile) => profile.id === routeId)) {
          return { status: "failed", message: "找不到要启用的线路" };
        }
        previewConfig = {
          ...previewConfig,
          settingsRevision: previewConfig.settingsRevision + 1,
          activeProfileId: routeId,
        };
        refreshPreviewModelState();
        return {
          status: "ok",
          config: previewConfig,
          modelState: previewModelState,
          ccSwitch: previewCcSwitch(),
          restartRequired: false,
          modelHotReloaded: true,
        };
      }
      if (command === "delete_route") {
        const routeId = String(args.routeId || "");
        const route = previewConfig.profiles.find((profile) => profile.id === routeId);
        if (!route) return { status: "failed", message: "找不到要删除的线路" };
        if (previewConfig.profiles.length <= 1) {
          return { status: "failed", message: "至少需要保留一条线路" };
        }
        const providerId = routeProviderId(route);
        const profiles = previewConfig.profiles.filter((profile) => profile.id !== routeId);
        delete previewConfig.selectedModelsByProvider[providerId];
        delete previewConfig.manualThirdPartyModelsByProvider[providerId];
        delete previewConfig.declaredOfficialModelsByProvider[providerId];
        delete previewConfig.upstreamModelsByProvider[providerId];
        delete previewConfig.defaultModelByProvider[providerId];
        previewConfig = {
          ...previewConfig,
          settingsRevision: previewConfig.settingsRevision + 1,
          profiles,
          activeProfileId:
            previewConfig.activeProfileId === routeId
              ? profiles[0].id
              : previewConfig.activeProfileId,
        };
        refreshPreviewModelState();
        return {
          status: "ok",
          config: previewConfig,
          modelState: previewModelState,
          ccSwitch: previewCcSwitch(),
          restartRequired: false,
          modelHotReloaded: true,
        };
      }
      if (command === "fetch_route_models") {
        const routeId = String(args.routeId || "");
        const route = previewConfig.profiles.find((profile) => profile.id === routeId);
        if (!route) return { status: "failed", message: "找不到要同步模型的线路" };
        const providerId = routeProviderId(route);
        const models = uniqueModelIds([
          ...previewUpstreamModels,
          ...(providerId === "backup" ? ["claude-sonnet-4-5"] : []),
        ]);
        previewConfig = {
          ...previewConfig,
          settingsRevision: previewConfig.settingsRevision + 1,
          upstreamModelsByProvider: {
            ...previewConfig.upstreamModelsByProvider,
            [providerId]: models,
          },
        };
        refreshPreviewModelState();
        return {
          status: "ok",
          config: previewConfig,
          modelState: previewModelState,
          routeModelState: previewModelStateForProfile(route),
          ccSwitch: previewCcSwitch(),
          models,
          restartRequired: false,
          modelHotReloaded: true,
        };
      }
      if (command === "sync_prompt_optimization_current_provider") {
        previewConfig = {
          ...previewConfig,
          settingsRevision: previewConfig.settingsRevision + 1,
          promptOptimization: {
            ...previewConfig.promptOptimization,
            baseUrl: previewCcSwitch().provider.baseUrl,
            apiKey: "",
            apiKeyConfigured: true,
            clearApiKey: false,
            model: previewModelState.defaultModel,
          },
        };
        return { config: previewConfig };
      }
      if (command === "clear_codex_trace_logs") {
        return {
          status: "ok",
          protectionEnabled: previewConfig.disableTraceLogWrites,
          cleanup: {
            databasesFound: 1,
            databasesCleaned: 1,
            rowsDeleted: 30141,
            bytesBefore: 406921216,
            bytesAfter: 49152,
            bytesReclaimed: 406872064,
          },
        };
      }
      if (command === "clear_diagnostic_storage") {
        previewTraceStats = {
          ...previewTraceLogStats,
          databaseBytes: 49152,
          rowCount: 0,
          estimatedLogBytes: 0,
        };
        previewCrashpadStats = {
          ...previewCrashpadPendingStats,
          protectionEnabled: previewConfig.protectCrashpadPending,
          reportsFound: 0,
          completeReports: 0,
          filesFound: 0,
          managedFiles: 0,
          pendingBytes: 0,
          managedBytes: 0,
        };
        return {
          status: "ok",
          traceProtectionEnabled: previewConfig.disableTraceLogWrites,
          traceLogWriteProtectionActive: previewConfig.disableTraceLogWrites,
          crashpadProtectionEnabled: previewConfig.protectCrashpadPending,
          errors: [],
          traceCleanup: {
            databasesFound: 1,
            databasesCleaned: 1,
            rowsDeleted: 30141,
            bytesBefore: 406921216,
            bytesAfter: 49152,
            bytesReclaimed: 406872064,
          },
          crashpadCleanup: {
            directoriesFound: 2,
            reportsFound: 13,
            reportsDeleted: 13,
            filesFound: 26,
            filesDeleted: 26,
            orphanFilesDeleted: 0,
            unmanagedFiles: 0,
            skippedRecentReports: 0,
            bytesBefore: 3448832,
            bytesAfter: 0,
            bytesReclaimed: 3448832,
            limitApplied: false,
            stillOverLimit: false,
            errors: [],
          },
          traceLogStats: previewTraceStats,
          crashpadPendingStats: previewCrashpadStats,
        };
      }
      if (command === "fetch_current_provider_models") {
        const activeProfile = activePreviewProfile();
        const providerId = activeProfile ? routeProviderId(activeProfile) : "primary";
        const declaredOfficialModels =
          previewConfig.declaredOfficialModelsByProvider[providerId] || [];
        const effectiveModels = uniqueModelIds([
          ...previewUpstreamModels,
          ...declaredOfficialModels,
        ]);
        previewModelState = {
          ...previewModelState,
          officialModels: previewOfficialModels.map((model) => ({
            ...model,
            supported: includesModelId(effectiveModels, model.slug),
          })),
          thirdPartyModels: previewModelState.thirdPartyModels.filter((model) =>
            includesModelId(effectiveModels, model)
          ),
          manualThirdPartyModels: previewModelState.manualThirdPartyModels.filter((model) =>
            !includesModelId(previewUpstreamModels, model)
          ),
          upstreamModels: effectiveModels,
        };
        return {
          status: "ok",
          config: previewConfig,
          models: previewUpstreamModels,
          modelState: previewModelState,
          restartRequired: false,
          modelHotReloaded: true,
        };
      }
      if (command === "save_selected_models") {
        const routeId = String(args.routeId || "");
        const targetProfile = previewConfig.profiles.find(
          (profile) => profile.id === routeId,
        ) || activePreviewProfile();
        const providerId = targetProfile ? routeProviderId(targetProfile) : "primary";
        const officialModels = (args.officialModels as string[]) || [];
        const thirdPartyModels = (args.thirdPartyModels as string[]) || [];
        const manualThirdPartyModels = (args.manualThirdPartyModels as string[]) || [];
        const supportedModels = uniqueModelIds([
          ...officialModels,
          ...thirdPartyModels,
        ]);
        previewConfig = {
          ...previewConfig,
          selectedModelsByProvider: {
            ...previewConfig.selectedModelsByProvider,
            [providerId]: thirdPartyModels,
          },
          manualThirdPartyModelsByProvider: {
            ...previewConfig.manualThirdPartyModelsByProvider,
            [providerId]: manualThirdPartyModels,
          },
          declaredOfficialModelsByProvider: {
            ...previewConfig.declaredOfficialModelsByProvider,
            [providerId]: officialModels,
          },
          upstreamModelsByProvider: {
            ...previewConfig.upstreamModelsByProvider,
            [providerId]: supportedModels,
          },
        };
        refreshPreviewModelState();
        return {
          status: "ok",
          config: previewConfig,
          modelState: previewModelState,
          restartRequired: false,
          modelHotReloaded: true,
        };
      }
      if (command === "save_default_model") {
        const model = String(args.model || "");
        const routeId = String(args.routeId || "");
        const targetProfile = previewConfig.profiles.find(
          (profile) => profile.id === routeId,
        ) || activePreviewProfile();
        const providerId = targetProfile ? routeProviderId(targetProfile) : "primary";
        previewConfig = {
          ...previewConfig,
          defaultModelByProvider: {
            ...previewConfig.defaultModelByProvider,
            [providerId]: model,
          },
        };
        if (targetProfile?.id === previewConfig.activeProfileId) {
          previewModelState = { ...previewModelState, defaultModel: model };
        }
        return {
          status: "ok",
          config: previewConfig,
          modelState: previewModelState,
          restartRequired: false,
          modelHotReloaded: true,
        };
      }
      if (command === "save_official_route_models") {
        const routeId = String(args.routeId || "");
        const models = uniqueModelIds((args.models as string[]) || []);
        const targetProfile = previewConfig.profiles.find(
          (profile) => profile.id === routeId,
        );
        if (!targetProfile || targetProfile.authMode !== "officialAccount" || models.length === 0) {
          return { status: "failed", message: "官方线路至少需要保留一个模型" };
        }
        const providerId = routeProviderId(targetProfile);
        const requestedDefault = previewConfig.defaultModelByProvider[providerId];
        const defaultModel = models.find((candidate) =>
          modelIdsEqual(candidate, requestedDefault || ""),
        ) || models[0];
        previewConfig = {
          ...previewConfig,
          selectedModelsByProvider: {
            ...previewConfig.selectedModelsByProvider,
            [providerId]: models,
          },
          defaultModelByProvider: {
            ...previewConfig.defaultModelByProvider,
            [providerId]: defaultModel,
          },
        };
        if (targetProfile.id === previewConfig.activeProfileId) {
          const selected = new Set(models.map(modelKey));
          previewModelState = {
            ...previewModelState,
            officialModels: previewOfficialModels.map((model) => ({
              ...model,
              supported: selected.has(modelKey(model.slug)),
            })),
            defaultModel,
          };
        }
        return {
          status: "ok",
          config: previewConfig,
          modelState: previewModelState,
          restartRequired: false,
          modelHotReloaded: true,
        };
      }
      if (command === "restart_codey") {
        return { status: "restarting" };
      }
      if (command === "check_for_updates") {
        return {
          currentVersion: "0.1.0",
          latestVersion: "0.2.0",
          updateAvailable: true,
          selectedAsset: {
            platform: "macos",
            arch: "arm64",
            packageType: "app-zip",
            fileName: "Codey-0.2.0-macos-arm64-unsigned.zip",
            url: "https://updates.example.com/releases/v0.2.0/Codey-0.2.0-macos-arm64-unsigned.zip",
            sha256:
              "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            size: 31_911_421,
          },
        };
      }
      if (command === "download_update") {
        return {
          latestVersion: "0.2.0",
          filePath: "/tmp/codey-updates/Codey-0.2.0-macos-arm64-unsigned.zip",
          fileName: "Codey-0.2.0-macos-arm64-unsigned.zip",
          size: 31_911_421,
          sha256:
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
          asset: {
            platform: "macos",
            arch: "arm64",
            packageType: "app-zip",
            fileName: "Codey-0.2.0-macos-arm64-unsigned.zip",
            url: "https://updates.example.com/releases/v0.2.0/Codey-0.2.0-macos-arm64-unsigned.zip",
            sha256:
              "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            size: 31_911_421,
          },
        };
      }
      if (command === "install_downloaded_update") {
        return { status: "installing" };
      }
      if (command === "test_webhook") {
        return { status: 200 };
      }
      if (command === "test_notification_channel") {
        const channel = args.channel as {
          kind?: string;
          url?: string;
          botToken?: string;
          chatId?: string;
        } | undefined;
        const configured = channel?.kind === "telegram"
          ? Boolean(channel.botToken?.trim() && channel.chatId?.trim())
          : Boolean(channel?.url?.trim());
        return configured
          ? { status: "ok", eventId: "preview-notification-test" }
          : { status: "failed", message: "请先完成渠道配置" };
      }
      if (command === "fetch_prompt_optimization_models") {
        return { models: previewModelState.upstreamModels };
      }
      if (command === "test_prompt_optimization") {
        return {
          status: "ok",
          result: { httpStatus: 200, responsePreview: "preview" },
        };
      }
      return { status: "ok" };
    };
  }
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <MantineProvider
      cssVariablesSelector="#root"
      forceColorScheme="light"
      theme={codeyMantineTheme}
    >
      <App />
    </MantineProvider>
  </React.StrictMode>,
);
