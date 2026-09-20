import type { ReactNode } from "react";

type SettingsPageHeaderProps = {
  id: string;
  title: string;
  description?: string;
  actions?: ReactNode;
  badge?: ReactNode;
  icon?: ReactNode;
};

export function SettingsPageHeader({
  id,
  title,
  description,
  actions,
  badge,
  icon,
}: SettingsPageHeaderProps) {
  return (
    <header className="settings-page-header">
      <div className="settings-page-header-main">
        {icon && (
          <div className="settings-page-icon-pill" aria-hidden="true">
            {icon}
          </div>
        )}
        <div className="settings-page-heading">
          <div className="settings-page-title-row">
            <h2 id={id}>{title}</h2>
            {badge}
          </div>
          {description && <p className="settings-page-description">{description}</p>}
        </div>
      </div>
      {actions && <div className="settings-page-actions">{actions}</div>}
    </header>
  );
}
