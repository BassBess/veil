import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ModelLabelEditor, type LabelResetPreview, type ModelConfiguration } from "./ModelLabelEditor";

type Step = { component: string; capability: string };
type Budget = { max_latency_ms: number; max_memory_mb: number; max_invocations: number };
type Profile = {
  id?: string;
  schema: 1;
  name: string;
  policy: "static";
  legacy_nlp: boolean;
  languages: string[];
  budget: Budget;
  steps: Step[];
};
type Adapter = { adapter: string; capability: string | null; languages?: string[]; labels?: string[] };
type Model = {
  id: string;
  name: string;
  version: string;
  license: string;
  size: number;
  loaded: boolean;
  configurable: boolean;
  compatibility_note?: string;
  adapters: Adapter[];
  files: Record<string, { size: number; sha256: string }>;
  source?: { repository: string; revision: string };
};
type Metric = {
  component: string;
  capability: string;
  invocations: number;
  failures: number;
  model_calls: number;
  latency_ms: number;
  memory_bytes: number | null;
  decision: string;
};
export type EngineStatus = {
  active_profile: string | null;
  custom_categories: string[];
  profiles: Profile[];
  models: Model[];
  algorithms: { id: string; name: string; capabilities: string[]; mandatory: boolean }[];
  last_run: { complete: boolean; metrics: Metric[] } | null;
};
type Download = {
  id: string;
  status: "running" | "cancelling" | "finished" | "cancelled" | "failed";
  progress: { received: number; total: number | null; file: string } | null;
  error: string | null;
};

const capabilities: Record<string, string> = {
  span_detection: "Sensitive span detection",
  candidate_generation: "Contextual candidate generation",
  contextual_classification: "Contextual classification",
  detection_fusion: "Detection fusion",
  entity_ranking: "Entity ranking",
  privacy_verification: "Privacy verification",
};
const languages: Record<string, string> = { en: "English", fr: "French", es: "Spanish", ar: "Arabic" };

function config<T>(action: string, id?: string, profile?: Omit<Profile, "id">) {
  return invoke<T>("engine_configuration", { action, id, profile });
}

function readEngineStatus() {
  return config<EngineStatus>("status");
}

function readableSize(size: number) {
  if (size < 1024) return `${size} B`;
  if (size < 1024 ** 2) return `${(size / 1024).toFixed(1)} KiB`;
  return `${(size / 1024 ** 2).toFixed(1)} MiB`;
}

function newProfile(): Profile {
  return {
    name: "", schema: 1, policy: "static", legacy_nlp: true,
    languages: ["en"],
    budget: { max_latency_ms: 170000, max_memory_mb: 0, max_invocations: 50000 },
    steps: [
      { component: "exact-span", capability: "detection_fusion" },
      { component: "veil-ranking", capability: "entity_ranking" },
    ],
  };
}

export function EnginePanel({ view, busy, onBusy, onProfileChanged, onStatus }: {
  view: "Model Library" | "Profiles";
  busy: boolean;
  onBusy: (busy: boolean) => void;
  onProfileChanged: () => void;
  onStatus: (status: EngineStatus) => void;
}) {
  const [status, setStatus] = useState<EngineStatus | null>(null);
  const [download, setDownload] = useState<Download | null>(null);
  const [source, setSource] = useState("");
  const [modelEditor, setModelEditor] = useState<{
    id: string; configuration: ModelConfiguration; resetPreview?: LabelResetPreview;
  } | null>(null);
  const [draft, setDraft] = useState<Profile | null>(null);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const downloading = download?.status === "running" || download?.status === "cancelling";

  const refresh = useCallback(async () => {
    const snapshot = await readEngineStatus();
    setStatus(snapshot);
    onStatus(snapshot);
  }, [onStatus]);

  useEffect(() => {
    let alive = true;
    void readEngineStatus().then(snapshot => {
      if (alive) { setStatus(snapshot); onStatus(snapshot); }
    }).catch(error => { if (alive) setError(String(error)); });
    return () => { alive = false; };
  }, [onStatus, view]);

  useEffect(() => {
    if (view !== "Model Library") return;
    let alive = true;
    let timer: number | undefined;
    let lastFinished = "";
    async function poll() {
      try {
        const job = await invoke<Download | null>("model_download_status");
        if (!alive) return;
        setDownload(job);
        if (job?.status === "finished" && job.id !== lastFinished) {
          lastFinished = job.id;
          void refresh().catch(error => { if (alive) setError(String(error)); });
        }
        const running = job?.status === "running" || job?.status === "cancelling";
        timer = window.setTimeout(() => void poll(), running ? 350 : 1000);
      } catch (error) {
        if (alive) setError(String(error));
      }
    }
    void poll();
    return () => { alive = false; window.clearTimeout(timer); };
  }, [refresh, view]);

  async function perform(action: () => Promise<unknown>, changed = false) {
    onBusy(true);
    setError("");
    setMessage("");
    try {
      await action();
      if (changed) onProfileChanged();
      await refresh();
    } catch (error) {
      setError(String(error));
    } finally {
      onBusy(false);
    }
  }

  async function localImport(kind: "model_file" | "model_directory" | "profile") {
    await perform(async () => {
      const result = await invoke<Model | Profile | null>("import_engine_file", { kind });
      if (result) setMessage(kind === "profile" ? "Profile imported." : "Model installed.");
    });
  }

  async function startDownload(event: React.FormEvent) {
    event.preventDefault();
    await perform(async () => {
      const job = await invoke<Download>("start_model_download", { source: source.trim(), authorized: true });
      setDownload(job);
    });
  }

  async function repairModel(model: Model) {
    await perform(async () => {
      const job = await invoke<Download>("start_model_download", { modelId: model.id, authorized: true });
      setModelEditor(null);
      setDownload(job);
    });
  }

  const active = status?.profiles.find(profile => profile.id === status.active_profile);

  if (view === "Model Library") {
    return <section className="view-details engine-panel" aria-label="Model Library">
      {error && <div className="error" role="alert">{error}</div>}
      {message && <p role="status">{message}</p>}
      <div className="engine-actions">
        <button className="secondary-button" disabled={busy} onClick={() => void localImport("model_file")}>Import model file or package</button>
        <button className="secondary-button" disabled={busy} onClick={() => void localImport("model_directory")}>Import model folder</button>
      </div>
      <form className="identity-form engine-download" onSubmit={startDownload}>
        <label>Hugging Face URL or repository ID
          <input value={source} onChange={event => setSource(event.target.value)} placeholder="owner/repository" maxLength={640} disabled={busy || downloading} />
        </label>
        <button className="primary" type="submit" disabled={busy || downloading || !source.trim()}>Download and install</button>
      </form>
      {download && <div className="engine-download-status" role="status">
        {downloading ? <>
          <span>{download.status === "cancelling" ? "Cancelling download…" : download.progress?.file || "Preparing download…"}</span>
          {download.progress && <span>{readableSize(download.progress.received)}{download.progress.total ? ` of ${readableSize(download.progress.total)}` : ""}</span>}
          <progress aria-label="Model download progress" max={download.progress?.total || 1}
            value={download.progress?.total ? download.progress.received : undefined} />
          <button className="secondary-button" disabled={download.status === "cancelling"}
            onClick={() => { void invoke("cancel_model_download").catch(error => setError(String(error))); }}>Cancel download</button>
        </> : download.status === "finished" ? "Model download complete." : download.status === "cancelled"
          ? "Model download cancelled." : download.error}
      </div>}
      <h2>Installed models</h2>
      {status && !status.models.length && <p>No models installed.</p>}
      {status?.models.map(model => {
        const supported = model.adapters.some(adapter => adapter.capability);
        const required = active?.steps.some(step => step.component === `model:${model.id}`);
        return <details key={model.id} className="engine-item">
          <summary><strong>{model.name}</strong><span>{readableSize(model.size)} · {model.loaded ? "Loaded" : supported ? "Installed" : model.configurable ? "Needs configuration" : "Unsupported"}</span></summary>
          <dl className="model-metadata">
            <dt>Library ID</dt><dd><code>{model.id}</code></dd>
            <dt>Version</dt><dd>{model.version}</dd>
            <dt>License</dt><dd>{model.license}</dd>
            {model.source && <><dt>Source</dt><dd>{model.source.repository} · <code>{model.source.revision}</code></dd></>}
            <dt>Capabilities</dt><dd>{model.adapters.map(adapter => adapter.capability
              ? `${capabilities[adapter.capability]} (${adapter.languages?.map(lang => languages[lang]).join(", ")})`
              : `${adapter.adapter} (unsupported)`).join(" · ") || "No compatible adapter"}</dd>
          </dl>
          {model.compatibility_note && <p>{model.compatibility_note}</p>}
          <details className="engine-file-list"><summary>Files and SHA-256 hashes</summary>
            <ul>{Object.entries(model.files).map(([name, file]) => <li key={name}><strong>{name}</strong> · {readableSize(file.size)}<code>{file.sha256}</code></li>)}</ul>
          </details>
          {modelEditor?.id === model.id && <ModelLabelEditor key={model.id}
            configuration={modelEditor.configuration} busy={busy || !!required}
            resetPreview={modelEditor.resetPreview}
            onPreviewReset={() => void perform(async () => {
              const resetPreview = await config<LabelResetPreview>("model_label_reset_preview", model.id);
              setModelEditor({ ...modelEditor, resetPreview });
            })}
            onCancelReset={() => setModelEditor({ ...modelEditor, resetPreview: undefined })}
            onConfirmReset={resetToken => void perform(async () => {
              await invoke("engine_configuration", { action: "model_label_reset", id: model.id, resetToken });
              setModelEditor(null);
              setMessage("Recommended mappings saved.");
            })}
            onClose={() => setModelEditor(null)}
            onSave={mappings => void perform(async () => {
              await invoke("engine_configuration", { action: "model_configure", id: model.id, mappings });
              setModelEditor(null);
              setMessage("Label mapping saved.");
            })} />}
          <div className="engine-actions">
            {model.source && <button className="secondary-button" disabled={busy || required || downloading}
              onClick={() => void repairModel(model)}>Repair from Hugging Face</button>}
            {model.configurable && <button className="secondary-button" disabled={busy || required || downloading}
              onClick={() => void perform(async () => {
                const configuration = await config<ModelConfiguration>("model_configuration", model.id);
                setModelEditor({ id: model.id, configuration });
              })}>Configure labels</button>}
            <button className="secondary-button" disabled={busy || !supported}
              onClick={() => void perform(() => config(model.loaded ? "model_unload" : "model_load", model.id))}>
              {model.loaded ? "Unload" : "Load and validate"}
            </button>
            <button className="secondary-button" disabled={busy || required}
              onClick={() => { if (window.confirm(`Remove ${model.name} from this device?`)) void perform(() => config("model_remove", model.id)); }}>Remove</button>
            {required && <span>Required by the active profile. Detection reloads it if unloaded.</span>}
          </div>
        </details>;
      })}
    </section>;
  }

  const optionsFor = (capability: string) => [
    ...(status?.algorithms ?? []).filter(item => !item.mandatory && item.capabilities.includes(capability)),
    ...(status?.models ?? []).filter(item => item.adapters.some(adapter => adapter.capability === capability))
      .map(item => ({ id: `model:${item.id}`, name: item.name })),
  ];
  const series = Object.keys(capabilities).slice(0, 5);
  function changeSteps(capability: string, components: string[]) {
    if (!draft) return;
    const steps = series.flatMap(task => task === capability
      ? components.map(component => ({ component, capability }))
      : draft.steps.filter(step => step.capability === task));
    setDraft({ ...draft, steps });
  }

  async function saveProfile(event: React.FormEvent) {
    event.preventDefault();
    if (!draft) return;
    const { id, ...profile } = draft;
    await perform(async () => {
      await config("profile_save", id, profile);
      setDraft(null);
      setMessage("Profile saved.");
    }, !!id && id === active?.id);
  }

  return <section className="view-details engine-panel" aria-label="Profiles">
    {error && <div className="error" role="alert">{error}</div>}
    {message && <p role="status">{message}</p>}
    <p className="active-engine-profile"><strong>Active detection</strong> {active?.name || "Veil’s existing detection"}</p>
    <div className="engine-actions">
      <button className="primary" disabled={busy || !status} onClick={() => setDraft(newProfile())}>Create profile</button>
      <button className="secondary-button" disabled={busy} onClick={() => void localImport("profile")}>Import profile</button>
      <button className="secondary-button" disabled={busy || !active}
        onClick={() => void perform(() => config("profile_activate"), true)}>Use existing detection</button>
    </div>
    {draft && <form className="settings-form profile-form" onSubmit={saveProfile}>
      <h2>{draft.id ? "Edit profile" : "New profile"}</h2>
      <fieldset disabled={busy}>
        <label>Profile name<input required maxLength={80} value={draft.name}
          onChange={event => setDraft({ ...draft, name: event.target.value })} /></label>
        <fieldset><legend>Document languages</legend>
          {Object.entries(languages).map(([value, name]) => <label key={value} className="engine-option">
            <input type="checkbox" checked={draft.languages.includes(value)} onChange={event => setDraft({ ...draft,
              languages: event.target.checked ? [...draft.languages, value] : draft.languages.filter(lang => lang !== value),
            })} />{name}</label>)}
        </fieldset>
        <label className="engine-option"><input type="checkbox" checked={draft.legacy_nlp}
          onChange={event => setDraft({ ...draft, legacy_nlp: event.target.checked })} />Include Veil’s legacy language models</label>
        <p>Shared pattern and checksum detection, saved phrases, and offset verification always run.</p>
        {series.map((capability, index) => {
          const options = optionsFor(capability);
          const selected = draft.steps.filter(step => step.capability === capability).map(step => step.component);
          if (index >= 3) return <label key={capability}>{capabilities[capability]}
            <select value={selected[0]} onChange={event => changeSteps(capability, [event.target.value])}>
              {options.map(option => <option key={option.id} value={option.id}>{option.name}</option>)}
            </select>
          </label>;
          const missing = selected.filter(component => !options.some(option => option.id === component));
          return <fieldset key={capability}><legend>{capabilities[capability]}</legend>
            {!options.length && !missing.length && <span>No compatible components installed.</span>}
            {[...options, ...missing.map(id => ({ id, name: `Missing component ${id.slice(0, 18)}…` }))].map(option =>
              <label className="engine-option" key={option.id}><input type="checkbox" checked={selected.includes(option.id)}
                onChange={event => changeSteps(capability, event.target.checked ? [...selected, option.id] : selected.filter(id => id !== option.id))} />{option.name}</label>)}
          </fieldset>;
        })}
        <fieldset><legend>Static execution limits</legend>
          <div className="engine-budget">
            {([
              ["max_latency_ms", "Latency (ms)", 1, 170000],
              ["max_memory_mb", "Process memory (MiB, 0 for no limit)", 0, 65536],
              ["max_invocations", "Component and model calls", 1, 50000],
            ] as const).map(([key, label, min, max]) => <label key={key}>{label}
              <input type="number" required min={min} max={max} value={draft.budget[key]}
                onChange={event => setDraft({ ...draft, budget: { ...draft.budget, [key]: event.target.valueAsNumber } })} />
            </label>)}
          </div>
        </fieldset>
        <div className="engine-actions">
          <button className="primary" type="submit">Save profile</button>
          <button className="secondary-button" type="button" onClick={() => setDraft(null)}>Cancel</button>
        </div>
      </fieldset>
    </form>}
    <h2>Saved profiles</h2>
    {status && !status.profiles.length && <p>No custom profiles saved.</p>}
    {status?.profiles.map(profile => <article className="engine-item profile-item" key={profile.id}>
      <div><strong>{profile.name}</strong><span>{profile.languages.map(lang => languages[lang]).join(", ")}{profile.id === active?.id ? " · Active" : ""}</span></div>
      <div className="engine-actions">
        <button className="secondary-button" disabled={busy || profile.id === active?.id}
          onClick={() => void perform(() => config("profile_activate", profile.id), true)}>Activate</button>
        <button className="secondary-button" disabled={busy} onClick={() => setDraft(profile)}>Edit</button>
        <button className="secondary-button" disabled={busy} onClick={() => setDraft({ ...profile, id: undefined, name: `${profile.name.slice(0, 75)} copy` })}>Duplicate</button>
        <button className="secondary-button" disabled={busy}
          onClick={() => void perform(async () => { if (await invoke("export_engine_profile", { id: profile.id })) setMessage("Profile exported."); })}>Export</button>
        <button className="secondary-button" disabled={busy || profile.id === active?.id}
          onClick={() => {
            if (window.confirm(`Delete the profile ${profile.name}?`)) void perform(async () => {
              await config("profile_delete", profile.id);
              if (draft?.id === profile.id) setDraft(null);
            });
          }}>Delete</button>
      </div>
    </article>)}
    {status?.last_run && <details className="engine-item run-metrics">
      <summary>Last detection · {status.last_run.complete ? "completed" : "incomplete"}</summary>
      {status.last_run.metrics.length ? <table>
        <thead><tr><th>Component</th><th>Task</th><th>Component calls</th><th>Model calls</th><th>Failures</th><th>Latency</th><th>Memory</th><th>Decision</th></tr></thead>
        <tbody>{status.last_run.metrics.map(item => <tr key={`${item.component}:${item.capability}`}>
          <th scope="row">{status.algorithms.find(algorithm => algorithm.id === item.component)?.name
            || status.models.find(model => `model:${model.id}` === item.component)?.name || item.component}</th>
          <td>{capabilities[item.capability] ?? item.capability}</td><td>{item.invocations}</td><td>{item.model_calls}</td><td>{item.failures}</td><td>{item.latency_ms.toFixed(1)} ms</td>
          <td>{item.memory_bytes === null ? "Unavailable" : readableSize(item.memory_bytes)}</td><td>{item.decision}</td>
        </tr>)}</tbody>
      </table> : <p>Detection stopped before the first component ran.</p>}
    </details>}
  </section>;
}
