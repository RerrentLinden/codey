import { useEffect, useId, useRef, useState } from "react";
import { Button, Checkbox, Input, Select } from "./components/ui";
import { applyPluginDefaults, initialPluginValue, samePluginValue, setPluginProperty, validatePluginConfig, type PluginSchema } from "./codeyPlugins";

const area = "min-h-28 w-full rounded-lg border border-gray-200 bg-transparent p-3 font-mono text-xs dark:border-gray-700";
type FieldProps = { schema: PluginSchema; value: unknown; onChange: (value: unknown) => void; disabled: boolean; name: string; required?: boolean; onDraftError: (path: string, invalid: boolean) => void; path: string };
function JsonEditor({ value, onChange, disabled, name, onDraftError }: FieldProps) {
  const id = useId();
  const serialized = JSON.stringify(value, null, 2) ?? "";
  const [text, setText] = useState(serialized);
  const [invalid, setInvalid] = useState(false);
  useEffect(() => { setText(serialized); setInvalid(false); onDraftError(id, false); }, [serialized, id, onDraftError]);
  useEffect(() => () => onDraftError(id, false), [id, onDraftError]);
  return <div className="grid gap-1"><p className="m-0 text-xs text-muted">此字段允许自由结构，请使用 JSON 编辑。</p><textarea className={area} aria-label={`${name} JSON`} disabled={disabled} value={text} onChange={event => {
    setText(event.target.value);
    try { const next = JSON.parse(event.target.value); onChange(next); setInvalid(false); onDraftError(id, false); }
    catch { setInvalid(true); onDraftError(id, true); }
  }} />{invalid && <p role="alert" className="m-0 text-xs text-red-600">请输入有效的 JSON</p>}</div>;
}
function ArrayFields(props: FieldProps & { value: unknown[] }) {
  const { schema, value, onChange, disabled, path, onDraftError } = props;
  const sequence = useRef(value.length);
  const [ids, setIds] = useState(() => value.map((_, index) => index));
  return <div className="grid gap-3 rounded-lg border border-gray-200 p-3 dark:border-gray-700">
    {!value.length && <p className="m-0 text-xs text-muted">尚未添加项目</p>}
    {value.map((item, index) => <div key={ids[index]} className="grid gap-2 border-b border-gray-200 pb-3 dark:border-gray-700">
      <Field schema={schema.items!} value={item} name={`第 ${index + 1} 项`} path={`${path}/${ids[index]}`} required disabled={disabled} onDraftError={onDraftError} onChange={next => onChange(value.map((entry, i) => i === index ? next : entry))} />
      <Button size="sm" variant="outline" className="justify-self-end" disabled={disabled} onClick={() => { setIds(previous => previous.filter((_, i) => i !== index)); onChange(value.filter((_, i) => i !== index)); }}>删除第 {index + 1} 项</Button>
    </div>)}
    <Button size="sm" variant="outline" className="justify-self-start" disabled={disabled || (schema.maxItems !== undefined && value.length >= schema.maxItems)} onClick={() => { const id = sequence.current++; setIds(previous => [...previous, id]); onChange([...value, initialPluginValue(schema.items!)]); }}>添加项目</Button>
  </div>;
}
function ExtraField({ value, properties, disabled, onChange }: { value: Record<string, unknown>; properties: Record<string, PluginSchema>; disabled: boolean; onChange: (value: unknown) => void }) {
  const [name, setName] = useState("");
  const key = name.trim();
  const occupied = Object.prototype.hasOwnProperty.call(value, key) || Object.prototype.hasOwnProperty.call(properties, key);
  return <details className="text-xs text-muted"><summary className="cursor-pointer">添加额外字段</summary>
    <div className="mt-2 flex gap-2"><Input aria-label="额外字段名称" disabled={disabled} value={name} onChange={event => setName(event.target.value)} />
      <Button size="sm" variant="outline" disabled={disabled || !key || occupied} onClick={() => { onChange(setPluginProperty(value, key, null)); setName(""); }}>添加字段</Button></div>
    {occupied && <p className="mb-0 text-red-600">字段名称已存在</p>}
  </details>;
}
function Field(props: FieldProps) {
  const { schema, value, onChange, disabled, name, required, path, onDraftError } = props;
  const label = schema.title || name;
  const errors = value === undefined ? required ? [`${label}：必填`] : [] : validatePluginConfig(value, schema, label);
  const object = value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
  const properties = schema.properties ?? {};
  const unknownKeys = Object.keys(object).filter(key => !Object.prototype.hasOwnProperty.call(properties, key));
  const visibleErrors = schema.type === "array" && schema.items && Array.isArray(value)
    ? errors.filter(error => !error.startsWith(`${label}[`))
    : schema.type === "object" && schema.properties && value && typeof value === "object" && !Array.isArray(value)
      ? errors.filter(error => !Object.keys(properties).some(key => ["：", ".", "["].some(separator => error.startsWith(`${label}.${key}${separator}`))))
      : errors;
  return <div className="grid min-w-0 gap-2">
    <div className="flex items-center justify-between gap-2"><span className="text-xs font-medium">{label}{required && <span className="ml-1 text-red-600">*</span>}</span>{!required && value !== undefined && <Button size="sm" variant="ghost" disabled={disabled} onClick={() => onChange(undefined)}>清除</Button>}</div>
    {value === undefined && !(required && ["string", "number", "integer"].includes(schema.type ?? "") && !schema.enum) ? <Button className="justify-self-start" size="sm" variant="outline" disabled={disabled} onClick={() => onChange(initialPluginValue(schema))}>设置{label}</Button>
      : schema.enum ? <Select aria-label={label} disabled={disabled} value={schema.enum.findIndex(item => samePluginValue(item, value)) + 1} optionList={schema.enum.map((item, index) => ({ value: index + 1, label: typeof item === "string" ? item : JSON.stringify(item) }))} onChange={selected => onChange(schema.enum![Number(selected) - 1])} />
      : schema.type === "object" && schema.properties ? <div className="grid gap-4 rounded-lg border border-gray-200 p-3 dark:border-gray-700">
        {Object.entries(properties).map(([key, child]) => <Field key={key} schema={child} name={key} path={`${path}/${key}`} required={schema.required?.includes(key)} disabled={disabled} onDraftError={onDraftError} value={Object.prototype.hasOwnProperty.call(object, key) ? object[key] : undefined} onChange={next => onChange(setPluginProperty(object, key, next))} />)}
        {unknownKeys.map(key => <Field key={key} schema={{ title: key }} name={key} path={`${path}/${key}`} value={object[key]} disabled={disabled} onDraftError={onDraftError} onChange={next => onChange(setPluginProperty(object, key, next))} />)}
        {schema.additionalProperties !== false && <ExtraField value={object} properties={properties} disabled={disabled} onChange={onChange} />}
      </div>
      : schema.type === "array" && schema.items && Array.isArray(value) ? <ArrayFields {...props} value={value} />
      : schema.type === "boolean" ? <Checkbox className="justify-self-start" aria-label={label} disabled={disabled} checked={value === true} onCheckedChange={next => onChange(next === true)} />
      : ["string", "number", "integer"].includes(schema.type ?? "") ? <Input aria-label={label} aria-invalid={errors.length > 0} disabled={disabled} type={schema.type === "string" ? "text" : "number"} step={schema.type === "integer" ? 1 : "any"} min={schema.minimum} max={schema.maximum} value={typeof value === "string" || typeof value === "number" ? value : ""} onChange={event => onChange(schema.type === "string" || event.target.value === "" ? event.target.value : Number(event.target.value))} />
      : schema.type === "null" ? <span className="text-xs text-muted">null</span> : <JsonEditor {...props} name={label} />}
    {schema.description && <p className="m-0 text-xs text-muted">{schema.description}</p>}
    {visibleErrors.length > 0 && <p role="alert" className="m-0 break-words text-xs text-red-600">{visibleErrors.join("；")}</p>}
  </div>;
}
export function SchemaConfigForm({ schema, value, disabled = false, onSave, onDraftChange }: { schema: PluginSchema; value: Record<string, unknown>; disabled?: boolean; onSave: (value: Record<string, unknown>) => void; onDraftChange?: (value: Record<string, unknown>, invalidDraft: boolean, changed: boolean) => void }) {
  const [config, setConfig] = useState(() => applyPluginDefaults(value, schema));
  const initial = useRef(config);
  const [draftErrors, setDraftErrors] = useState<Record<string, boolean>>({});
  const [onDraftError] = useState(() => (path: string, invalid: boolean) => setDraftErrors(previous => previous[path] === invalid ? previous : { ...previous, [path]: invalid }));
  const rootSchema = { ...schema, type: "object" };
  const invalid = Object.values(draftErrors).some(Boolean) || validatePluginConfig(config, rootSchema).length > 0;
  const latestDraftChange = useRef(onDraftChange); latestDraftChange.current = onDraftChange;
  useEffect(() => { latestDraftChange.current?.(config as Record<string, unknown>, Object.values(draftErrors).some(Boolean), !samePluginValue(config, initial.current)); }, [config, draftErrors]);
  return <div className="grid gap-4"><Field schema={rootSchema} name="配置" path="config" required value={config} onChange={setConfig} disabled={disabled} onDraftError={onDraftError} /><div><Button size="sm" disabled={disabled || invalid} onClick={() => onSave(config as Record<string, unknown>)}>保存配置</Button></div></div>;
}
