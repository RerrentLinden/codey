import { useMemo, useRef } from "react";
import {
  IconCheck,
  IconCirclePlus,
  IconRobot,
  IconX,
} from "@tabler/icons-react";

import { inputShellClass, insetInputClass } from "../uiClasses";
import { Combobox, useCombobox } from "./mantine";

export type ManualModelComboboxProps = {
  ariaDescribedBy?: string;
  ariaInvalid?: boolean;
  ariaLabel?: string;
  disabled?: boolean;
  getPopupContainer?: () => HTMLElement;
  id?: string;
  onChange: (value: string) => void;
  options: string[];
  placeholder?: string;
  value: string;
  zIndex?: number;
};

export function ManualModelCombobox({
  ariaDescribedBy,
  ariaInvalid,
  ariaLabel = "模型",
  disabled = false,
  getPopupContainer,
  id,
  onChange,
  options,
  placeholder = "例如 gpt-4o-mini 或 deepseek-chat",
  value,
  zIndex,
}: ManualModelComboboxProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const combobox = useCombobox({
    onDropdownClose: () => combobox.resetSelectedOption(),
  });

  const query = value.trim().toLowerCase();
  const trimmedValue = value.trim();

  const matchedOptions = useMemo(() => {
    if (!query) return options;
    return options.filter((opt) => opt.toLowerCase().includes(query));
  }, [options, query]);

  const hasExactMatch = useMemo(() => {
    if (!query) return false;
    return options.some((opt) => opt.toLowerCase() === query);
  }, [options, query]);

  const isCustomValue = Boolean(trimmedValue) && !hasExactMatch;
  const portalTarget = getPopupContainer?.();

  return (
    <Combobox
      middlewares={{ flip: true, shift: true }}
      onOptionSubmit={(selectedVal) => {
        onChange(selectedVal);
        combobox.closeDropdown();
      }}
      portalProps={portalTarget ? { target: portalTarget } : undefined}
      position="bottom-start"
      store={combobox}
      withinPortal={Boolean(portalTarget)}
      zIndex={zIndex}
    >
      <Combobox.Target>
        <div
          className={
            inputShellClass +
            " w-full flex-1 cursor-text" +
            (disabled ? " cursor-not-allowed opacity-60" : "")
          }
          onClick={() => {
            if (!disabled) {
              inputRef.current?.focus();
              combobox.openDropdown();
            }
          }}
        >
          <IconRobot size={15} aria-hidden="true" className="text-[#7d7d83]" />
          <Combobox.EventsTarget>
            <input
              ref={inputRef}
              aria-describedby={ariaDescribedBy}
              aria-invalid={ariaInvalid}
              aria-label={ariaLabel}
              autoComplete="off"
              className={insetInputClass + " font-medium"}
              disabled={disabled}
              id={id}
              onChange={(e) => {
                onChange(e.target.value);
                combobox.openDropdown();
                combobox.updateSelectedOptionIndex();
              }}
              onClick={(e) => {
                e.stopPropagation();
                combobox.openDropdown();
              }}
              onFocus={() => {
                combobox.openDropdown();
              }}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  combobox.closeDropdown();
                }
              }}
              placeholder={placeholder}
              spellCheck={false}
              type="text"
              value={value}
            />
          </Combobox.EventsTarget>
          {value && !disabled ? (
            <button
              aria-label="清空模型"
              className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-[#8e8e93] transition-colors hover:bg-black/5 hover:text-[#1d1d1f]"
              onClick={(e) => {
                e.stopPropagation();
                onChange("");
                inputRef.current?.focus();
                combobox.openDropdown();
              }}
              type="button"
            >
              <IconX size={13} aria-hidden="true" />
            </button>
          ) : null}
          <button
            aria-label="切换模型下拉列表"
            className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-[#8e8e93] transition-colors hover:bg-black/5 hover:text-[#1d1d1f]"
            disabled={disabled}
            onClick={(e) => {
              e.stopPropagation();
              combobox.toggleDropdown();
              inputRef.current?.focus();
            }}
            tabIndex={-1}
            type="button"
          >
            <Combobox.Chevron size="xs" />
          </button>
        </div>
      </Combobox.Target>

      <Combobox.Dropdown
        className="w-[var(--combobox-target-width)] min-w-[280px] max-w-[calc(100vw-32px)] overflow-hidden rounded-[10px]! border-black/10! p-1! shadow-[0_12px_32px_rgba(0,0,0,0.14)]!"
      >
        <Combobox.Options className="max-h-[260px] overflow-y-auto py-0.5">
          {isCustomValue ? (
            <Combobox.Option
              active={trimmedValue === value}
              className="mx-0.5 mb-1 rounded-[7px] px-2.5 py-1.5 text-xs data-[combobox-selected]:bg-blue-500/9 data-[combobox-selected]:text-[#1d1d1f]"
              value={trimmedValue}
            >
              <div className="flex min-w-0 items-center justify-between gap-2">
                <div className="flex min-w-0 items-center gap-1.5 truncate">
                  <IconCirclePlus
                    aria-hidden="true"
                    className="shrink-0 text-blue-500"
                    size={14}
                  />
                  <span className="shrink-0 text-[#6e6e73]">使用自定义模型</span>
                  <span className="truncate font-semibold text-[#1d1d1f]">
                    {trimmedValue}
                  </span>
                </div>
                <span className="grid w-4 shrink-0 place-items-center text-blue-600">
                  <IconCheck aria-hidden="true" size={14} />
                </span>
              </div>
            </Combobox.Option>
          ) : null}

          {matchedOptions.length > 0
            ? matchedOptions.map((option) => {
                const isSelected = option === trimmedValue;
                return (
                  <Combobox.Option
                    active={isSelected}
                    className="mx-0.5 rounded-[7px] px-2.5 py-1.5 text-xs data-[combobox-selected]:bg-blue-500/9 data-[combobox-selected]:text-[#1d1d1f]"
                    key={option}
                    value={option}
                  >
                    <div className="flex min-w-0 items-center justify-between gap-2">
                      <span className="truncate font-medium text-[#1d1d1f]">
                        {option}
                      </span>
                      <span className="grid w-4 shrink-0 place-items-center text-blue-600">
                        {isSelected ? (
                          <IconCheck aria-hidden="true" size={14} />
                        ) : null}
                      </span>
                    </div>
                  </Combobox.Option>
                );
              })
            : options.length > 0
              ? (
                <Combobox.Group
                  label={
                    <span className="block px-2 py-1 text-[11px] font-medium text-[#8e8e93]">
                      已获取的模型列表（共 {options.length} 个）
                    </span>
                  }
                >
                  {options.map((option) => {
                    const isSelected = option === trimmedValue;
                    return (
                      <Combobox.Option
                        active={isSelected}
                        className="mx-0.5 rounded-[7px] px-2.5 py-1.5 text-xs data-[combobox-selected]:bg-blue-500/9 data-[combobox-selected]:text-[#1d1d1f]"
                        key={option}
                        value={option}
                      >
                        <div className="flex min-w-0 items-center justify-between gap-2">
                          <span className="truncate font-medium text-[#1d1d1f]">
                            {option}
                          </span>
                          <span className="grid w-4 shrink-0 place-items-center text-blue-600">
                            {isSelected ? (
                              <IconCheck aria-hidden="true" size={14} />
                            ) : null}
                          </span>
                        </div>
                      </Combobox.Option>
                    );
                  })}
                </Combobox.Group>
              )
              : !isCustomValue
                ? (
                  <Combobox.Empty className="py-4 text-center text-xs text-[#8e8e93]">
                    暂无模型列表，可输入自定义模型或点击「获取列表」
                  </Combobox.Empty>
                )
                : null}
        </Combobox.Options>
      </Combobox.Dropdown>
    </Combobox>
  );
}
