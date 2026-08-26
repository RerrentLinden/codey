import { useMemo, useRef, useState } from "react";
import { IconAlertTriangle, IconCheck, IconSearch } from "@tabler/icons-react";

import type { SubagentModelOption } from "../subagentModels";
import { resolveSubagentModelOption } from "../subagentModels";
import { compactSelectInputClass } from "../uiClasses";
import { Combobox, InputBase, useCombobox } from "./mantine";

type ModelComboboxProps = {
  "aria-label": string;
  disabled?: boolean;
  getPopupContainer?: () => HTMLElement;
  onChange: (value: string) => void;
  options: SubagentModelOption[];
  placeholder?: string;
  preferredProviderId?: string;
  value: string;
  zIndex?: number;
};

type ModelOptionGroup = {
  key: string;
  routeName: string;
  providerId: string;
  options: SubagentModelOption[];
};

function normalizedSearchText(value: string) {
  return value.trim().toLocaleLowerCase();
}

export function ModelCombobox({
  "aria-label": ariaLabel,
  disabled = false,
  getPopupContainer,
  onChange,
  options,
  placeholder = "请选择模型",
  preferredProviderId,
  value,
  zIndex,
}: ModelComboboxProps) {
  const [search, setSearch] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const combobox = useCombobox({
    onDropdownClose: () => {
      combobox.resetSelectedOption();
      setSearch("");
    },
    onDropdownOpen: () => {
      requestAnimationFrame(() => searchInputRef.current?.focus());
    },
  });
  const selectedOption = useMemo(
    () => resolveSubagentModelOption(options, value, preferredProviderId),
    [options, preferredProviderId, value],
  );
  const validValues = useMemo(
    () => new Set(options.map((option) => option.value)),
    [options],
  );
  const searchableOptions = useMemo(
    () =>
      options.map((option) => ({
        option,
        searchText: [
          option.label,
          option.modelId,
          option.routeName,
          option.routePrefix,
          option.providerId,
        ]
          .map(normalizedSearchText)
          .join("\u0000"),
      })),
    [options],
  );
  const normalizedSearch = normalizedSearchText(search);
  const filteredOptions = useMemo(
    () =>
      normalizedSearch
        ? searchableOptions
            .filter(({ searchText }) => searchText.includes(normalizedSearch))
            .map(({ option }) => option)
        : options,
    [normalizedSearch, options, searchableOptions],
  );
  const groups = useMemo(
    () => {
      const grouped = new Map<string, ModelOptionGroup>();
      for (const option of filteredOptions) {
        const existing = grouped.get(option.routeId);
        if (existing) {
          existing.options.push(option);
        } else {
          grouped.set(option.routeId, {
            key: option.routeId,
            routeName: option.routeName,
            providerId: option.providerId,
            options: [option],
          });
        }
      }
      return [...grouped.values()];
    },
    [filteredOptions],
  );
  const portalTarget = getPopupContainer?.();
  const unavailableValue = value.trim() && !selectedOption ? value.trim() : "";
  const triggerText = selectedOption
    ? `[${selectedOption.routePrefix}] ${selectedOption.label}`
    : unavailableValue
      ? `${unavailableValue} · 已不可用`
      : placeholder;

  return (
    <Combobox
      classNames={{
        dropdown:
          "w-[360px]! max-w-[calc(100vw-24px)]! overflow-hidden rounded-[10px]! border-black/10! p-0! shadow-[0_12px_32px_rgba(0,0,0,0.14)]!",
        option:
          "mx-1! rounded-[7px]! px-2.5! py-2! text-xs data-[combobox-selected]:bg-blue-500/9! data-[combobox-selected]:text-[#1d1d1f]!",
      }}
      middlewares={{ flip: true, shift: true }}
      onOptionSubmit={(nextValue) => {
        if (!validValues.has(nextValue)) return;
        onChange(nextValue);
        combobox.closeDropdown();
      }}
      portalProps={portalTarget ? { target: portalTarget } : undefined}
      position="bottom-start"
      store={combobox}
      withinPortal={Boolean(portalTarget)}
      zIndex={zIndex}
    >
      <Combobox.Target>
        <InputBase
          aria-invalid={Boolean(unavailableValue) || undefined}
          aria-label={ariaLabel}
          className="w-full min-w-0"
          classNames={{
            input: `${compactSelectInputClass} flex! items-center pr-8! text-left!`,
            section: "text-[#6e6e73]",
          }}
          component="button"
          disabled={disabled}
          onClick={() => combobox.toggleDropdown()}
          pointer
          rightSection={
            unavailableValue
              ? <IconAlertTriangle size={14} color="#b7791f" aria-hidden="true" />
              : <Combobox.Chevron />
          }
          rightSectionPointerEvents="none"
          title={triggerText}
          type="button"
        >
          <span
            className={
              unavailableValue
                ? "block min-w-0 truncate text-[#9a6700]"
                : selectedOption
                  ? "block min-w-0 truncate text-[#3a3a3c]"
                  : "block min-w-0 truncate text-[#8e8e93]"
            }
          >
            {triggerText}
          </span>
        </InputBase>
      </Combobox.Target>

      <Combobox.Dropdown>
        <Combobox.Search
          ref={searchInputRef}
          aria-label={`搜索${ariaLabel}`}
          classNames={{
            input:
              "h-9! rounded-none! border-0! border-b! border-black/7! bg-[#f8f8fa]! pl-9! text-xs! focus:ring-0!",
          }}
          leftSection={<IconSearch size={14} aria-hidden="true" />}
          onChange={(event) => setSearch(event.currentTarget.value)}
          placeholder="搜索模型或线路"
          value={search}
        />
        <Combobox.Options className="max-h-[280px] overflow-y-auto py-1.5">
          {groups.map((group) => (
            <Combobox.Group
              key={group.key}
              label={
                <span className="flex min-w-0 items-center justify-between gap-2 px-1 text-[10px] font-semibold text-[#8e8e93]">
                  <span className="truncate">{group.routeName}</span>
                  <span className="shrink-0 font-mono font-normal text-[#aeaeb2]">
                    {group.providerId}
                  </span>
                </span>
              }
            >
              {group.options.map((option) => {
                const selected = selectedOption?.value === option.value;
                return (
                  <Combobox.Option
                    active={selected}
                    key={option.value}
                    value={option.value}
                  >
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="grid min-w-0 flex-1 gap-0.5">
                        <span className="truncate font-semibold text-[#3a3a3c]">
                          {option.label}
                        </span>
                        <span className="truncate text-[10px] text-[#8e8e93]">
                          {option.routeName} · {option.modelId}
                        </span>
                      </span>
                      <span className="grid w-4 shrink-0 place-items-center text-blue-600">
                        {selected && <IconCheck size={14} aria-hidden="true" />}
                      </span>
                    </span>
                  </Combobox.Option>
                );
              })}
            </Combobox.Group>
          ))}
          {groups.length === 0 && (
            <Combobox.Empty className="py-6 text-xs text-[#8e8e93]">
              {options.length === 0 ? "还没有可用于子代理的模型" : "没有匹配的模型或线路"}
            </Combobox.Empty>
          )}
        </Combobox.Options>
      </Combobox.Dropdown>
    </Combobox>
  );
}
