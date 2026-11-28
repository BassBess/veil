import { useState } from "react";

const ignore = "@ignore";
const custom = "@custom";
type Mapping = Record<string, string>;
export type LabelMappings = Record<string, Mapping>;
export type LabelResetPreview = {
  token: string;
  applicable: boolean;
  changes: { adapter: string; label: string; current: string | null; recommended: string }[];
};
export type ModelConfiguration = {
  categories: string[];
  can_reset: boolean;
  adapters: {
    adapter: string;
    groups: {
      label: string;
      automatic: string | null;
      suggested: string;
      reason: "taxonomy" | "approved" | "custom" | "conflict";
    }[];
    overrides: Mapping;
    missing: string[];
  }[];
};

const reasons = {
  taxonomy: "Shared taxonomy",
  approved: "Previously approved",
  custom: "Suggested custom category",
  conflict: "Previous approvals disagree",
};

export function ModelLabelEditor({ configuration, busy, onSave, onClose, resetPreview,
  onPreviewReset, onCancelReset, onConfirmReset }: {
  configuration: ModelConfiguration;
  busy: boolean;
  onSave: (mappings: LabelMappings) => void;
  onClose: () => void;
  resetPreview?: LabelResetPreview;
  onPreviewReset: () => void;
  onCancelReset: () => void;
  onConfirmReset: (token: string) => void;
}) {
  const [mappings, setMappings] = useState<LabelMappings>(() =>
    Object.fromEntries(configuration.adapters.map(adapter => [adapter.adapter, adapter.overrides])));

  function update(adapter: string, label: string, value: string) {
    const next = { ...mappings[adapter] };
    if (value === "") delete next[label];
    else next[label] = value;
    setMappings({ ...mappings, [adapter]: next });
  }

  function applySuggestions() {
    const next = Object.fromEntries(configuration.adapters.map(adapter => [
      adapter.adapter,
      { ...Object.fromEntries(adapter.groups.map(group => [group.label, group.suggested])),
        ...mappings[adapter.adapter] },
    ]));
    onSave(next);
  }

  if (resetPreview) {
    return <section className="settings-form label-editor" aria-labelledby="label-reset-title">
      <h3 id="label-reset-title">Review recommended mappings</h3>
      {resetPreview.changes.length > 0 ? <>
        <p>Confirm to apply the shared taxonomy to all labels. This replaces the saved assignments shown below and discards edits in this form.</p>
        <div className="label-reset-changes">
          <table aria-label="Proposed label mapping changes">
            <thead><tr><th>Native label</th><th>Current mapping</th><th>Recommended mapping</th></tr></thead>
            <tbody>{resetPreview.changes.map(change => <tr key={`${change.adapter}:${change.label}`}>
              <th scope="row">{change.label}<small>{change.adapter}</small></th>
              <td>{change.current === ignore ? "Ignored" : change.current ?? "Unresolved"}</td>
              <td>{change.recommended}</td>
            </tr>)}</tbody>
          </table>
        </div>
      </> : <p>{resetPreview.applicable
        ? "Confirm to save the recommended mappings for this adapter and discard edits in this form."
        : "Saved mappings already match the recommendations."}</p>}
      <div className="engine-actions">
        {resetPreview.applicable && <button className="primary" type="button" disabled={busy}
          onClick={() => onConfirmReset(resetPreview.token)}>Confirm reset</button>}
        <button className="secondary-button" type="button" disabled={busy}
          onClick={onCancelReset}>Back to label mapping</button>
      </div>
    </section>;
  }

  return <form className="settings-form label-editor" onSubmit={event => { event.preventDefault(); onSave(mappings); }}>
    <h3>Model label mapping</h3>
    <p>Apply Veil’s suggestions in one step or edit individual labels. Applying suggestions keeps your manual choices. Saved sensitive mappings can be reused for future imports.</p>
    <fieldset disabled={busy}>
      <div className="engine-actions">
        <button className="primary" type="button" onClick={applySuggestions}>Apply all suggested mappings</button>
        <button className="secondary-button" type="button" disabled={!configuration.can_reset} onClick={onPreviewReset}>
          Reset to recommended mappings
        </button>
      </div>
      {!configuration.can_reset && <p>This installation lacks the original model labels needed for a reset.</p>}
      {configuration.adapters.map(adapter => {
        const decisions = mappings[adapter.adapter];
        const unresolved = adapter.groups.filter(group => !decisions[group.label] && !group.automatic).length;
        const ignored = adapter.groups.filter(group => (decisions[group.label] ?? group.automatic) === ignore).length;
        return <section key={adapter.adapter} aria-label={adapter.adapter}>
          <div className="engine-actions">
            <strong>{adapter.adapter}</strong>
            <span>{unresolved} unresolved · {ignored} ignored</span>
          </div>
          {adapter.missing.length > 0 && <p>Files needed for activation · {adapter.missing.join(", ")}</p>}
          <div className="label-editor-grid">
            {adapter.groups.map(group => {
              const value = decisions[group.label] ?? "";
              const customCode = value.startsWith("CUSTOM_") && !configuration.categories.includes(value);
              const automatic = group.automatic === ignore ? "Ignore" : group.automatic;
              return <div className="label-editor-row" key={group.label}>
                <label>{group.label}
                  <select value={customCode ? custom : value} onChange={event => update(adapter.adapter, group.label,
                    event.target.value === custom
                      ? group.suggested.startsWith("CUSTOM_") && !configuration.categories.includes(group.suggested)
                        ? group.suggested : "CUSTOM_LABEL"
                      : event.target.value)}>
                    <option value="">{automatic ? `Automatic · ${automatic}` : "Unresolved"}</option>
                    {configuration.categories.map(category => <option key={category}>{category}</option>)}
                    <option value={custom}>Custom category</option>
                    <option value={ignore}>Ignore explicitly</option>
                  </select>
                </label>
                {customCode && <label>Custom category code for {group.label}
                  <input required value={value.slice(7)} maxLength={32} pattern="[A-Z][A-Z0-9]*(_[A-Z0-9]+)*"
                    onChange={event => update(adapter.adapter, group.label, `CUSTOM_${event.target.value.toUpperCase()}`)} />
                </label>}
                <small>{reasons[group.reason]} · {group.suggested}</small>
              </div>;
            })}
          </div>
        </section>;
      })}
      <p>Explicit ignores apply to this model. Custom categories appear in Anonymize.</p>
      <div className="engine-actions">
        <button className="primary" type="submit">Save label mapping</button>
        <button className="secondary-button" type="button" onClick={onClose}>Cancel</button>
      </div>
    </fieldset>
  </form>;
}
