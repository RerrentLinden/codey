import { memo, useEffect, useMemo, useState } from "react";
import {
  IconAlertTriangle as AlertTriangle,
  IconCheck as Check,
  IconFolderOpen as FolderOpen,
  IconLoader2 as LoaderCircle,
  IconPlus as Plus,
  IconRefresh as RefreshCw,
  IconTrash as Trash2,
} from "@tabler/icons-react";

import type { Confirmation, ModelState } from "./App.types";
import {
  Badge,
  Button,
  Checkbox,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
} from "./components/semi";

type ModelPickerDialogProps = {
  open: boolean;
  isBusy: boolean;
  busy: string | null;
  container: HTMLElement | null;
  customModelInput: string;
  modelInputError: string;
  modelSyncWarning: string;
  thirdPartyModelOptions: string[];
  modelState: ModelState;
  draftModelSet: Set<string>;
  manualThirdPartyModelKeys: Set<string>;
  onOpenChange: (open: boolean) => void;
  onCustomModelInputChange: (model: string) => void;
  onAddCustomModel: () => void;
  onToggleDraftModel: (model: string, checked: boolean) => void;
  onDeleteThirdPartyModel: (model: string) => void;
  onSave: () => void;
};

const MODEL_PICKER_PAGE_SIZE = 200;

function ModelPickerDialogComponent({
  open,
  isBusy,
  busy,
  container,
  customModelInput,
  modelInputError,
  modelSyncWarning,
  thirdPartyModelOptions,
  modelState,
  draftModelSet,
  manualThirdPartyModelKeys,
  onOpenChange,
  onCustomModelInputChange,
  onAddCustomModel,
  onToggleDraftModel,
  onDeleteThirdPartyModel,
  onSave,
}: ModelPickerDialogProps) {
  const [modelQuery, setModelQuery] = useState("");
  const [visibleThirdPartyCount, setVisibleThirdPartyCount] = useState(
    MODEL_PICKER_PAGE_SIZE,
  );
  useEffect(() => {
    if (!open) return;
    setModelQuery("");
    setVisibleThirdPartyCount(MODEL_PICKER_PAGE_SIZE);
  }, [open]);
  const filteredThirdPartyModels = useMemo(() => {
    if (!open) return [];
    const query = modelQuery.trim().toLowerCase();
    if (!query) return thirdPartyModelOptions;
    return thirdPartyModelOptions.filter((model) =>
      model.toLowerCase().includes(query)
    );
  }, [modelQuery, open, thirdPartyModelOptions]);
  const visibleThirdPartyModels = filteredThirdPartyModels.slice(
    0,
    visibleThirdPartyCount,
  );
  const selectedThirdPartyModelKeys = useMemo(
    () =>
      open
        ? new Set(modelState.thirdPartyModels.map((model) => model.trim().toLowerCase()))
        : new Set<string>(),
    [modelState.thirdPartyModels, open],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {open && <DialogContent
        className="model-picker-dialog"
        container={container}
        onEscapeKeyDown={(event) => {
          if (isBusy) event.preventDefault();
        }}
        onPointerDownOutside={(event) => {
          if (isBusy) event.preventDefault();
        }}
      >
        <DialogHeader>
          <DialogTitle>配置当前线路支持的模型</DialogTitle>
          <DialogDescription>
            默认展示 7 个官方模型，请按线路实际能力勾选；其他模型可输入模型 ID 手动添加。
          </DialogDescription>
        </DialogHeader>
        {modelSyncWarning && (
          <div className="model-picker-warning" role="alert">
            <AlertTriangle size={17} aria-hidden="true" />
            <span>{modelSyncWarning}</span>
          </div>
        )}
        <div className="model-picker-add">
          <div className="input-shell">
            <Input
              value={customModelInput}
              onChange={(event) => onCustomModelInputChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  onAddCustomModel();
                }
              }}
              placeholder="输入其他模型 ID，例如 provider-model-v2"
              spellCheck={false}
              aria-label="输入其他模型 ID"
              aria-invalid={Boolean(modelInputError)}
              disabled={isBusy}
            />
          </div>
          <Button
            variant="secondary"
            size="sm"
            disabled={isBusy || !customModelInput.trim()}
            onClick={onAddCustomModel}
          >
            <Plus aria-hidden="true" />
            添加
          </Button>
        </div>
        {modelInputError && (
          <p className="model-picker-input-error" role="alert">{modelInputError}</p>
        )}
        <div className="model-picker-list">
          <div className="model-picker-list-heading">
            <div>
              <strong>官方模型</strong>
              <small>勾选当前第三方线路实际支持的模型</small>
            </div>
            <Badge variant="info">{modelState.officialModels.length} 个</Badge>
          </div>
          {modelState.officialModels.map((model) => (
            <div className="model-picker-row official" key={model.slug}>
              <Checkbox
                checked={draftModelSet.has(model.slug)}
                disabled={isBusy}
                onCheckedChange={(checked) =>
                  onToggleDraftModel(model.slug, checked === true)}
                aria-label={`当前线路支持 ${model.slug}`}
              />
              <div className="model-picker-model-copy">
                <strong>{model.displayName}</strong>
                <small>{model.slug}</small>
              </div>
              <Badge variant="info">官方模型</Badge>
            </div>
          ))}
          <div className="model-picker-list-heading other-models">
            <div>
              <strong>其他模型</strong>
              <small>可选择同步发现的模型，也可在上方手动输入</small>
            </div>
            <Badge variant="secondary">
              {filteredThirdPartyModels.length === thirdPartyModelOptions.length
                ? `${thirdPartyModelOptions.length} 个`
                : `${filteredThirdPartyModels.length} / ${thirdPartyModelOptions.length} 个`}
            </Badge>
          </div>
          {thirdPartyModelOptions.length > 0 && (
            <div className="model-picker-search">
              <Input
                value={modelQuery}
                onChange={(event) => {
                  setModelQuery(event.target.value);
                  setVisibleThirdPartyCount(MODEL_PICKER_PAGE_SIZE);
                }}
                placeholder="搜索其他模型"
                spellCheck={false}
                aria-label="搜索其他模型"
                disabled={isBusy}
              />
            </div>
          )}
          {visibleThirdPartyModels.map((model) => {
            const added =
              draftModelSet.has(model) ||
              selectedThirdPartyModelKeys.has(model.trim().toLowerCase());
            const manual = manualThirdPartyModelKeys.has(model.trim().toLowerCase());
            return (
              <div className="model-picker-row" key={model}>
                <Checkbox
                  checked={draftModelSet.has(model)}
                  disabled={isBusy}
                  onCheckedChange={(checked) => onToggleDraftModel(model, checked === true)}
                  aria-label={`当前线路支持 ${model}`}
                />
                <span className="model-picker-model-id">{model}</span>
                {added && manual && (
                  <Button
                    variant="ghost"
                    size="xs"
                    className="model-picker-delete-button"
                    disabled={isBusy}
                    onClick={() => onDeleteThirdPartyModel(model)}
                    aria-label={`删除其他模型 ${model}`}
                  >
                    <Trash2 aria-hidden="true" />
                    删除
                  </Button>
                )}
              </div>
            );
          })}
          {visibleThirdPartyModels.length < filteredThirdPartyModels.length && (
            <div className="model-picker-load-more">
              <Button
                variant="ghost"
                size="sm"
                disabled={isBusy}
                onClick={() =>
                  setVisibleThirdPartyCount((count) =>
                    Math.min(
                      count + MODEL_PICKER_PAGE_SIZE,
                      filteredThirdPartyModels.length,
                    )
                  )}
              >
                再显示{" "}
                {Math.min(
                  MODEL_PICKER_PAGE_SIZE,
                  filteredThirdPartyModels.length - visibleThirdPartyModels.length,
                )}{" "}
                个
              </Button>
            </div>
          )}
          {filteredThirdPartyModels.length === 0 && (
            <div className="empty-state">
              {thirdPartyModelOptions.length === 0
                ? "尚无其他模型，可在上方输入模型 ID 添加"
                : "没有匹配的其他模型"}
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={isBusy} onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button
            disabled={isBusy}
            onClick={onSave}
          >
            {busy === "save-models"
              ? <LoaderCircle className="spinner" aria-hidden="true" />
              : <Check aria-hidden="true" />}
            保存模型支持情况
          </Button>
        </DialogFooter>
      </DialogContent>}
    </Dialog>
  );
}

type ConfirmationDialogProps = {
  confirmation: Confirmation | null;
  container: HTMLElement | null;
  onClose: () => void;
  onConfirm: (confirmation: Confirmation) => void;
};

function ConfirmationDialogComponent({
  confirmation,
  container,
  onClose,
  onConfirm,
}: ConfirmationDialogProps) {
  const destructive =
    confirmation?.action === "clear" ||
    confirmation?.action === "delete-notification-channel";
  return (
    <Dialog open={Boolean(confirmation)} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="confirmation-dialog" container={container}>
        <DialogHeader>
          <DialogTitle>{confirmation?.title}</DialogTitle>
          <DialogDescription>{confirmation?.description}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>取消</Button>
          <Button
            variant={
              destructive
                ? "destructive"
                : confirmation?.action === "restart"
                  ? "warning"
                  : "default"
            }
            onClick={() => {
              if (confirmation) onConfirm(confirmation);
            }}
          >
            {destructive
              ? <Trash2 aria-hidden="true" />
              : confirmation?.action === "restart"
              ? <RefreshCw aria-hidden="true" />
              : <Check aria-hidden="true" />}
            {confirmation?.confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

type CodexAppPathDialogProps = {
  open: boolean;
  selectedPath: string;
  error: string;
  isBusy: boolean;
  busy: string | null;
  container: HTMLElement | null;
  onOpenChange: (open: boolean) => void;
  onChooseDirectory: () => void;
  onConfirm: () => void;
};

function CodexAppPathDialogComponent({
  open,
  selectedPath,
  error,
  isBusy,
  busy,
  container,
  onOpenChange,
  onChooseDirectory,
  onConfirm,
}: CodexAppPathDialogProps) {
  const choosing = busy === "pick-codex-app-directory";
  const confirming = busy === "set-codex-app-path";
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="codex-app-path-dialog"
        container={container}
        onEscapeKeyDown={(event) => {
          if (isBusy) event.preventDefault();
        }}
        onPointerDownOutside={(event) => {
          if (isBusy) event.preventDefault();
        }}
      >
        <DialogHeader>
          <DialogTitle>未找到 Codex 桌面应用</DialogTitle>
          <DialogDescription>
            自动扫描没有发现 Codex。请选择 Codex 桌面应用所在的目录；确认时会校验目录及可执行文件。
          </DialogDescription>
        </DialogHeader>
        <div className="codex-app-path-selection">
          <span>所选目录</span>
          <code>{selectedPath || "尚未选择"}</code>
        </div>
        {error && <p className="codex-app-path-error" role="alert">{error}</p>}
        <DialogFooter>
          <Button variant="outline" disabled={isBusy} onClick={onChooseDirectory}>
            {choosing
              ? <LoaderCircle className="spinner" aria-hidden="true" />
              : <FolderOpen aria-hidden="true" />}
            选择目录
          </Button>
          <Button disabled={isBusy || !selectedPath} onClick={onConfirm}>
            {confirming
              ? <LoaderCircle className="spinner" aria-hidden="true" />
              : <Check aria-hidden="true" />}
            确认并启动
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export const ModelPickerDialog = memo(ModelPickerDialogComponent);
export const ConfirmationDialog = memo(ConfirmationDialogComponent);
export const CodexAppPathDialog = memo(CodexAppPathDialogComponent);
