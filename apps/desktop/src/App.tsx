import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import { rebaseDetections } from "./rebaseDetections";

type Detection = {
  entity_type: string;
  text: string;
  start: number;
  end: number;
  confidence: number;
};

type ReviewStatus = "pending" | "accepted" | "rejected";

type ReviewedDetection = Omit<Detection, "confidence"> & {
  id: string;
  status: ReviewStatus;
  confidence: number | null;
  needsCorrection?: boolean;
};

type DetectionResponse = {
  detections: Detection[];
};

type ValidatedDetection = Pick<
  ReviewedDetection,
  "id" | "entity_type" | "text" | "start" | "end"
>;

type ParsedDocument = {
  text: string;
  file_name: string;
  file_type: string;
  pages: number | null;
  warnings: string[];
};

function importErrorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error && typeof error.message === "string") return error.message;
  if (typeof error === "string") return error;
  return "The document could not be imported. Please try another file.";
}

type PrivacyCategory = {
  label: string;
  value: string;
};

type ManualSelection = {
  start: number;
  end: number;
  text: string;
};

const PRIVACY_CATEGORIES: PrivacyCategory[] = [
  { label: "Person names", value: "PERSON" },
  { label: "Email addresses", value: "EMAIL_ADDRESS" },
  { label: "Phone numbers", value: "PHONE_NUMBER" },
  { label: "Locations", value: "LOCATION" },
  { label: "Organizations", value: "ORGANIZATION" },
];

function rangesOverlap(
  first: { start: number; end: number },
  second: { start: number; end: number }
) {
  return first.start < second.end && first.end > second.start;
}

type IconName = "document" | "upload" | "home" | "database" | "vault" | "settings" | "arrow" | "plus" | "edit" | "trash";

function Icon({ name }: { name: IconName }) {
  const paths: Record<IconName, React.ReactNode> = {
    document: <><path d="M14 3H5v18h14V8Z" /><path d="M14 3v5h5M8 12h8M8 16h6" /></>,
    upload: <><path d="M12 16V3m-5 5 5-5 5 5M4 16v5h16v-5" /></>,
    home: <><path d="m3 10 9-7 9 7" /><path d="M5 9v12h5v-7h4v7h5V9" /></>,
    database: <><ellipse cx="12" cy="5" rx="8" ry="3" /><path d="M4 5v14c0 4 16 4 16 0V5M4 12c0 4 16 4 16 0" /></>,
    vault: <><rect x="3" y="3" width="18" height="18" rx="3" /><circle cx="12" cy="11" r="3" /><path d="M12 14v3" /></>,
    settings: <><path d="m9 3-1 3-3 1 1 3-2 2 2 2-1 3 3 1 1 3h6l1-3 3-1-1-3 2-2-2-2 1-3-3-1-1-3Z" /><circle cx="12" cy="12" r="3" /></>,
    arrow: <><path d="M4 12h16m-6-6 6 6-6 6" /></>,
    plus: <path d="M12 5v14M5 12h14" />,
    edit: <><path d="m15 4 5 5M4 20l5-1L21 7a2 2 0 0 0-5-5L4 14Z" /></>,
    trash: <><path d="M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7m4-7v7" /></>,
  };
  return <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name]}</svg>;
}

function App() {

  const [workspaceView, setWorkspaceView] = useState("Anonymize");
  const [reviewQuery, setReviewQuery] = useState("");
  const [reviewFilter, setReviewFilter] = useState("all");
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [selectionHint, setSelectionHint] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [text, setText] = useState(
    "John Smith lives in New York. Contact him at john.smith@example.com."
  );

  const [detections, setDetections] = useState<ReviewedDetection[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [analyzed, setAnalyzed] = useState(false);
  const [reviewError, setReviewError] = useState("");
  const [documentEdited, setDocumentEdited] = useState(false);
  const [importedDocument, setImportedDocument] = useState<ParsedDocument | null>(null);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState("");

  const [selectedEntities, setSelectedEntities] = useState<string[]>(
    PRIVACY_CATEGORIES.map((category) => category.value)
  );

  const [manualSelection, setManualSelection] =
    useState<ManualSelection | null>(null);
  const [manualCategory, setManualCategory] = useState("PERSON");
  const [manualError, setManualError] = useState("");

  const [validation, setValidation] = useState<{
    snapshot: string;
    success: boolean;
    message: string;
  } | null>(null);

  const reviewSnapshot = JSON.stringify([text, detections]);


  const editingDetection = detections.find((item) => item.id === editingId);

  function cancelEdit() {
    setEditingId(null);
    setManualSelection(null);
    setReviewError("");
  }

  function startEdit(item: ReviewedDetection) {
    setEditingId(item.id);
    setManualSelection(null);
    setSelectionHint(false);
    setReviewError("");
    setManualError("");
    const input = inputRef.current;
    if (input) {
      input.focus();
      // Convert code-point offsets back to the textarea's UTF-16 offsets.
      const chars = Array.from(text);
      input.setSelectionRange(chars.slice(0, item.start).join("").length, chars.slice(0, item.end).join("").length);
      input.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  }

  function toggleEntity(entity: string) {
    if (loading) {
      return;
    }

    setSelectedEntities((current) =>
      current.includes(entity)
        ? current.filter((item) => item !== entity)
        : [...current, entity]
    );
  }

  function updateText(value: string) {
    if (value === text) return;
    if (importedDocument) {
      setDetections([]);
      setAnalyzed(false);
      setDocumentEdited(false);
    } else {
      setDetections((current) => rebaseDetections(text, value, current));
      if (analyzed) setDocumentEdited(true);
    }
    setText(value);
    setEditingId(null);
    setSelectionHint(false);
    setError("");
    setValidation(null);
    setReviewError("");
    setManualError("");
    setManualSelection(null);
  }

  function replaceDocumentText(value: string) {
    setText(value);
    setDetections([]);
    setAnalyzed(false);
    setDocumentEdited(false);
    setEditingId(null);
    setSelectionHint(false);
    setManualSelection(null);
    setValidation(null);
    setError("");
    setReviewError("");
    setManualError("");
    setImportError("");
  }

  async function uploadDocument() {
    if (loading) return;
    setLoading(true);
    setImporting(true);
    setImportError("");
    try {
      const document = await invoke<ParsedDocument | null>("parse_document");
      // Cancellation and failures preserve the current text and review decisions.
      if (document) {
        replaceDocumentText(document.text);
        setImportedDocument(document);
      }
    } catch (error) {
      setImportError(importErrorMessage(error));
    } finally {
      setLoading(false);
      setImporting(false);
    }
  }

  function removeDocument() {
    if (loading) return;
    setImportedDocument(null);
    replaceDocumentText("");
    inputRef.current?.focus();
  }

  function addManualDetection() {
    if (!manualSelection || !analyzed || loading) {
      return;
    }

    const overlaps = detections.some(
      (item) =>
        item.status !== "rejected" &&
        rangesOverlap(manualSelection, item)
    );

    if (overlaps) {
      setManualError(
        "This selection overlaps an existing detection. Reject that detection first if you want to replace it."
      );
      return;
    }

    const newDetection: ReviewedDetection = {
      id: crypto.randomUUID(),
      text: manualSelection.text,
      start: manualSelection.start,
      end: manualSelection.end,
      entity_type: manualCategory,
      confidence: null,
      status: "pending",
    };

    setDetections((current) =>
      [...current, newDetection].sort(
        (first, second) => first.start - second.start
      )
    );

    setManualSelection(null);
    setManualError("");
    setReviewError("");
  }

  function updateDetectionBoundary(id: string) {
    if (!manualSelection || !analyzed || loading) {
      return;
    }

    const target = detections.find((item) => item.id === id);

    if (!target) {
      return;
    }

    const overlaps = detections.some(
      (item) =>
        item.id !== id &&
        item.status !== "rejected" &&
        rangesOverlap(manualSelection, item)
    );

    if (overlaps) {
      setReviewError(
        "The selected text overlaps another active detection. Reject that detection first."
      );
      return;
    }

    setDetections((current) =>
      current
        .map((item) =>
          item.id === id
            ? {
                ...item,
                text: manualSelection.text,
                start: manualSelection.start,
                end: manualSelection.end,
                confidence: null,
                needsCorrection: false,
                status: "pending" as ReviewStatus,
              }
            : item
        )
        .sort((first, second) => first.start - second.start)
    );

    setEditingId(null);
    setManualSelection(null);
    setManualError("");
    setReviewError("");
  }

  function updateDetectionCategory(id: string, entityType: string) {
    if (loading) {
      return;
    }

    setReviewError("");
    setManualError("");

    setDetections((current) =>
      current.map((item) =>
        item.id === id
          ? {
              ...item,
              entity_type: entityType,
              status:
                item.status === "rejected" ? "rejected" : "pending",
            }
          : item
      )
    );
  }

  function updateReviewStatus(id: string, status: ReviewStatus) {
    if (loading) {
      return;
    }

    const target = detections.find((item) => item.id === id);

    if (!target) {
      return;
    }

    if (status === "accepted" && target.needsCorrection) {
      setReviewError("This text was removed from the document. Edit this item to select a new range, or reject it.");
      return;
    }

    const overlaps = detections.some(
      (item) =>
        item.id !== id &&
        item.status !== "rejected" &&
        rangesOverlap(target, item)
    );

    if (status !== "rejected" && overlaps) {
      setReviewError(
        "This detection overlaps another active detection. Reject the other detection first."
      );
      return;
    }

    setReviewError("");
    setManualError("");

    setDetections((current) =>
      current.map((item) =>
        item.id === id ? { ...item, status } : item
      )
    );
  }

  async function detectPII() {
    if (loading || !text.trim() || selectedEntities.length === 0) {
      return;
    }

    setLoading(true);
    setError("");
    setValidation(null);
    setReviewError("");
    setManualError("");
    setManualSelection(null);
    setDetections([]);
    setAnalyzed(false);
    setEditingId(null);
    setSelectionHint(false);

    try {
      const response = await invoke<DetectionResponse>("detect_pii", {
        text,
        entities: selectedEntities,
      });

      setDetections(
        response.detections.map((item, index) => ({
          ...item,
          id: `detection-${index}`,
          status: "pending",
        }))
      );

      setAnalyzed(true);
      setDocumentEdited(false);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }


  async function validateAcceptedDetections() {
    if (loading || !analyzed) return;

    const accepted: ValidatedDetection[] = detections
      .filter((item) => item.status === "accepted")
      .map(({ id, entity_type, text, start, end }) => ({
        id,
        entity_type,
        text,
        start,
        end,
      }));

    if (accepted.length === 0) return;

    if (detections.some((item) => item.status === "pending")) {
      setValidation({
        snapshot: reviewSnapshot,
        success: false,
        message: "Please accept or reject all detections before validation.",
      });
      return;
    }

    const snapshot = reviewSnapshot;
    setLoading(true);
    setValidation(null);

    try {
      const result = await invoke<ValidatedDetection[]>(
        "validate_reviewed_detections",
        { text, detections: accepted }
      );

      setValidation({
        snapshot,
        success: true,
        message: `${result.length} accepted item(s) validated.`,
      });
    } catch (err) {
      setValidation({
        snapshot,
        success: false,
        message: `Validation failed: ${String(err)}`,
      });
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="app" aria-busy={loading}>
      <header className="sidebar">
        <div className="brand">
          <div className={`vault-mark vault-reactive ${workspaceView === "Vault" ? "is-open" : ""}`} aria-hidden="true">
            <div className="vault-door"><span className="vault-wheel"><i /><i /></span><span className="vault-handle" /></div>
          </div>
          <span>Veil</span>
        </div>
        <nav aria-label="Main navigation">
          {["Anonymize", "Pseudonymize", "Vault", "Settings"].map((view) => (
            <button key={view} className={`nav-item ${workspaceView === view ? "active" : ""}`} aria-current={workspaceView === view ? "page" : undefined} onClick={() => setWorkspaceView(view)}>
              <Icon name={view === "Vault" ? "vault" : view === "Settings" ? "settings" : view === "Pseudonymize" ? "database" : "home"} />{view}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom"><span className="local-dot" />Processed locally on your device</div>
        <div className="sidebar-wave" aria-hidden="true" />
      </header>

      <main className="workspace">
        <header className="intro">
          <h1>{workspaceView === "Anonymize" ? "Anonymization workspace" : workspaceView === "Pseudonymize" ? "Pseudonymization workspace" : workspaceView}</h1>
          <p>{workspaceView === "Vault" ? "Records of processed people and sensitive information." : workspaceView === "Settings" ? "Manage your detection preferences." : "Inspect your documents and review sensitive information before sharing."}</p>
        </header>

        {(workspaceView === "Vault" || workspaceView === "Settings") && <section className="view-details">
          {workspaceView === "Vault" && <><div className="vault-display" aria-hidden="true"><div className="vault-mark is-open"><div className="vault-door"><span className="vault-wheel"><i /><i /></span><span className="vault-handle" /></div></div></div><h2>Your local vault</h2><p>No records to display.</p><button className="primary" onClick={() => setWorkspaceView("Anonymize")}>Open document workspace</button></>}
          {workspaceView === "Settings" && <><h2>Detection categories</h2><p>Choose which information Veil should look for.</p><div className="privacy-categories">{PRIVACY_CATEGORIES.map((category) => <label className="privacy-category" key={category.value}><input type="checkbox" disabled={loading} checked={selectedEntities.includes(category.value)} onChange={() => toggleEntity(category.value)} />{category.label}</label>)}</div></>}
        </section>}
        <div hidden={workspaceView === "Vault" || workspaceView === "Settings"}>
        <div className={`mode-indicator ${workspaceView === "Pseudonymize" ? "pseudonymize-mode" : ""}`} role="status" aria-live="polite">
          <Icon name={workspaceView === "Pseudonymize" ? "database" : "home"} />
          <div><strong>{workspaceView === "Pseudonymize" ? "Pseudonymization" : "Anonymization"}</strong>
          <p>{workspaceView === "Pseudonymize" ? "Reversible replacement using pseudonyms and a secure identity mapping." : "Irreversible removal or replacement of identifying information."}</p>
          <span>Current step: detect and review sensitive information.</span></div>
        </div>
        <section aria-label="Document and detection settings" className="input-section" id="document">
          <div className="input-methods" aria-label="Document input method">
            <button className={`input-method ${!importedDocument ? "selected" : ""}`} aria-pressed={!importedDocument} disabled={loading} onClick={() => {
              setImportedDocument(null);
              setImportError("");
              inputRef.current?.focus();
            }}><Icon name="document" />Paste text</button>
            <button className={`input-method ${importedDocument ? "selected" : ""}`} aria-pressed={!!importedDocument} disabled={loading} onClick={uploadDocument}><Icon name="upload" />{importing ? "Opening / parsing locally…" : "Upload document"}</button>
            <span className="upload-help">TXT, DOCX, PDF, CSV, XLSX · up to 20 MB · local only</span>
          </div>
          {importedDocument && (
            <div className="file-summary">
              <div><strong>{importedDocument.file_name}</strong><span>{importedDocument.file_type.toUpperCase()}{importedDocument.pages !== null ? ` · ${importedDocument.pages} page(s)` : ""}</span></div>
              <button className="secondary-button" disabled={loading} onClick={uploadDocument}><Icon name="upload" />Replace file</button>
              <button className="secondary-button" disabled={loading} onClick={removeDocument}><Icon name="trash" />Remove file</button>
            </div>
          )}
          {importing && <p className="selection-hint" role="status">Choose a document in the local file picker. Its text will be extracted on this device.</p>}
          {importError && <div className="error" role="alert">{importError}</div>}
          {importedDocument?.warnings.map((warning, index) => <div className="document-warning" role="status" key={index}>{warning}</div>)}
          <div className="document-input" aria-busy={importing}>
            <span className="input-size" id="input-size">{text.length.toLocaleString()} / 100,000 characters</span>
          <textarea
            ref={inputRef}
            aria-label="Document input"
            value={text}
            onChange={(event) => updateText(event.target.value)}
            disabled={loading}
            aria-describedby={editingDetection ? "input-size edit-instructions" : "input-size"}
            maxLength={100000}
            placeholder="Paste your text here…"
            onSelect={(event) => {
              const input = event.currentTarget;
              const start = input.selectionStart;
              const end = input.selectionEnd;
              const selectedText = input.value.slice(start, end);
              setManualError("");
              if (!analyzed || loading || !selectedText.trim()) {
                setManualSelection(null);
                return;
              }
              setSelectionHint(false);
              setManualSelection({
                start: Array.from(input.value.slice(0, start)).length,
                end: Array.from(input.value.slice(0, end)).length,
                text: selectedText,
              });
            }}
          />
          </div>
          {editingDetection && (
            <div className="edit-panel" aria-label="Correct detection">
              <div id="edit-instructions" role="status">
                <strong>Editing “{editingDetection.text}”</strong>
                <p>Select the corrected text in the document above, then save your correction.</p>
                <p>Replacement: <strong>{manualSelection?.text ?? "Select text above"}</strong></p>
              </div>
              <div className="edit-actions">
                <button className="secondary-button" onClick={cancelEdit} disabled={loading}>Cancel</button>
                <button className="primary" disabled={loading || !manualSelection} onClick={() => updateDetectionBoundary(editingDetection.id)}>Save correction</button>
              </div>
              {reviewError && <div className="error" role="alert">{reviewError}</div>}
            </div>
          )}
          <div className="detect-toolbar">
            <span className="detect-label">Detect</span>
            <div className="privacy-categories">
              {PRIVACY_CATEGORIES.map((category, index) => (
                <label key={category.value} className="privacy-category">
                  <input type="checkbox" disabled={loading} checked={selectedEntities.includes(category.value)} onChange={() => toggleEntity(category.value)} />
                  <span>{["Names", "Emails", "Phones", "Locations", "Organizations"][index]}</span>
                </label>
              ))}
            </div>
            <button className="primary detect-button" onClick={detectPII} disabled={loading || !text.trim() || selectedEntities.length === 0}>
              {!importing && loading && !analyzed ? "Detecting…" : "Detect"}<Icon name="arrow" />
            </button>
          </div>
          {selectedEntities.length === 0 && <div className="error" role="alert">Select at least one sensitive data category.</div>}
          {error && <div className="error" role="alert">Detection failed: {error}</div>}
        </section>

        <section className="results-panel" id="review" aria-labelledby="results-heading">
          <div className="panel-header">
            <div><h2 id="results-heading">Detected Information</h2><p>Review and refine the detected items before continuing.</p></div>
            <span className="result-count">{detections.filter((item) => item.status !== "pending").length} / {detections.length} reviewed</span>
          </div>
          {documentEdited && <p className="selection-hint" role="status">Your review has been preserved. Items affected by text changes need review again. Newly added information is not automatically detected; select it to add it manually.</p>}
          {reviewError && !editingDetection && <div className="error" role="alert">{reviewError}</div>}
          <div className="review-toolbar">
            <input type="search" aria-label="Search detected information" placeholder="Search detected information" value={reviewQuery} onChange={(event) => setReviewQuery(event.target.value)} />
            <select aria-label="Filter review status" value={reviewFilter} onChange={(event) => setReviewFilter(event.target.value)}>
              <option value="all">All detections</option>
              <option value="pending">Pending review</option>
              <option value="accepted">Accepted</option>
              <option value="rejected">Rejected</option>
            </select>
            <button className="secondary-button" disabled={!reviewQuery && reviewFilter === "all"} onClick={() => { setReviewQuery(""); setReviewFilter("all"); }}>Clear filters</button>
          </div>
          {detections.length > 0 && !detections.some((item) => (reviewFilter === "all" || item.status === reviewFilter) && item.text.toLowerCase().includes(reviewQuery.toLowerCase())) && <p role="status" className="selection-hint">No detections match these filters.</p>}
          {detections.length === 0 ? (
            <div className="empty-results">{analyzed ? "No sensitive information detected. Select text above to add missed information." : "Your detected information will appear here. Add your text and select Detect to get started."}</div>
          ) : (
            <div className="detection-list">
              {detections.filter((item) => (reviewFilter === "all" || item.status === reviewFilter) && item.text.toLowerCase().includes(reviewQuery.toLowerCase())).map((item) => (
                <div className={`detection-item ${item.status} ${editingId === item.id ? "editing" : ""}`} key={item.id}>

                  <div className="entity-text"><strong>{item.text}</strong><span className="review-status">{item.needsCorrection && item.status !== "rejected" ? "Text removed — edit or reject" : item.status === "pending" ? "Pending review" : item.status === "rejected" ? "Rejected" : "Accepted"}</span></div>
                  <select aria-label={`Category for ${item.text}`} value={item.entity_type} disabled={loading || editingId !== null} onChange={(event) => updateDetectionCategory(item.id, event.target.value)}>
                    {PRIVACY_CATEGORIES.map((category) => <option key={category.value} value={category.value}>{category.label}</option>)}
                  </select>
                  <span className="confidence">{item.confidence === null ? "Manual" : `${(item.confidence * 100).toFixed(0)}%`}</span>
                  <div className="row-actions">
                    <button className="secondary-button edit-button" aria-label={`Edit ${item.text}`} disabled={loading || editingId !== null} onClick={() => startEdit(item)}><Icon name="edit" />Edit</button>
                    <button className={`review-button accept-button ${item.status === "accepted" ? "selected" : ""}`} aria-pressed={item.status === "accepted"} aria-label={`Accept ${item.text}`} disabled={loading || editingId !== null || item.status === "accepted"} onClick={() => updateReviewStatus(item.id, "accepted")}>Accept</button>
                    <button className={`review-button reject-button ${item.status === "rejected" ? "selected" : ""}`} aria-pressed={item.status === "rejected"} aria-label={`Reject ${item.text}`} disabled={loading || editingId !== null || item.status === "rejected"} onClick={() => updateReviewStatus(item.id, "rejected")}>Reject</button>
                    {item.status !== "pending" && <button className="reset-button" disabled={loading || editingId !== null} onClick={() => updateReviewStatus(item.id, "pending")} aria-label={`Reset review for ${item.text}`}>Reset</button>}
                  </div>
                </div>
              ))}
            </div>
          )}
          <button className="add-button" disabled={!analyzed || loading || editingId !== null} onClick={() => { setSelectionHint(true); inputRef.current?.focus(); }}><Icon name="plus" />Add missed information</button>
          {selectionHint && <p className="selection-hint" role="status">Select the missed information in the document above, choose a category, then select Add information.</p>}
          {analyzed && manualSelection && !editingId && (
            <div className="manual-detection">
              <p>Selected text: <strong>{manualSelection.text}</strong></p>
              <label>Category <select value={manualCategory} disabled={loading} onChange={(event) => { setManualCategory(event.target.value); setManualError(""); }}>{PRIVACY_CATEGORIES.map((category) => <option key={category.value} value={category.value}>{category.label}</option>)}</select></label>
              <button className="primary" onClick={addManualDetection} disabled={loading}>Add information<Icon name="plus" /></button>
              {manualError && <div className="error" role="alert">{manualError}</div>}
            </div>
          )}
          <div className="continue-actions">
            {analyzed && detections.some((item) => item.status === "pending") && <p>Accept or reject each item to continue.</p>}
            <button className="primary continue-button" title="Validate accepted detections" onClick={validateAcceptedDetections} disabled={loading || editingId !== null || !detections.some((item) => item.status === "accepted")}>
              {!importing && loading && analyzed ? "Validating…" : "Validate review"}<Icon name="arrow" />
            </button>
          </div>
          {validation?.snapshot === reviewSnapshot && <div className={validation.success ? "validation-success" : "error"} role={validation.success ? "status" : "alert"}>{validation.message}</div>}
        </section>
        </div>
      </main>
    </div>
  );
}

export default App;
