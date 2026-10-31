import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

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
};

type DetectionResponse = {
  detections: Detection[];
};

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

function App() {
  const [text, setText] = useState(
    "John Smith lives in New York. Contact him at john.smith@example.com."
  );

  const [detections, setDetections] = useState<ReviewedDetection[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [analyzed, setAnalyzed] = useState(false);
  const [reviewError, setReviewError] = useState("");

  const [selectedEntities, setSelectedEntities] = useState<string[]>(
    PRIVACY_CATEGORIES.map((category) => category.value)
  );

  const [manualSelection, setManualSelection] =
    useState<ManualSelection | null>(null);
  const [manualCategory, setManualCategory] = useState("PERSON");
  const [manualError, setManualError] = useState("");

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
    setText(value);
    setDetections([]);
    setAnalyzed(false);
    setError("");
    setReviewError("");
    setManualError("");
    setManualSelection(null);
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
                status: "pending" as ReviewStatus,
              }
            : item
        )
        .sort((first, second) => first.start - second.start)
    );

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
    setReviewError("");
    setManualError("");
    setManualSelection(null);
    setDetections([]);
    setAnalyzed(false);

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
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <div className="logo">V</div>

          <div>
            <h1>Veil</h1>
            <p>Privacy starts here.</p>
          </div>
        </div>

        <div className="status">
          <span>●</span>
          Local Processing
        </div>
      </header>

      <main className="workspace">
        <section className="intro">
          <p className="eyebrow">PRIVACY WORKSPACE</p>
          <h2>Protect what matters.</h2>

          <p className="intro-text">
            Detect sensitive information in your text before sharing it with
            others.
          </p>
        </section>

        <section className="panel">
          <div className="panel-header">
            <div>
              <h3>Document Input</h3>
              <p>Paste the text you want Veil to inspect.</p>
            </div>

            <span className="character-count">
              {text.length.toLocaleString()} / 100,000
            </span>
          </div>

          <textarea
            aria-label="Document input"
            value={text}
            onChange={(event) => updateText(event.target.value)}
            disabled={loading}
            maxLength={100000}
            placeholder="Paste your text here..."
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

              // Match Python's Unicode code-point positions.
              setManualSelection({
                start: Array.from(input.value.slice(0, start)).length,
                end: Array.from(input.value.slice(0, end)).length,
                text: selectedText,
              });
            }}
          />

          {analyzed && manualSelection && (
            <div className="manual-detection">
              <p>
                Selected text: <strong>{manualSelection.text}</strong>
              </p>

              <label>
                Category{" "}
                <select
                  value={manualCategory}
                  onChange={(event) => {
                    setManualCategory(event.target.value);
                    setManualError("");
                  }}
                >
                  {PRIVACY_CATEGORIES.map((category) => (
                    <option key={category.value} value={category.value}>
                      {category.label}
                    </option>
                  ))}
                </select>
              </label>

              <button
                type="button"
                onClick={addManualDetection}
                disabled={loading}
              >
                Add missed PII
              </button>

              {manualError && (
                <div className="error" role="alert">
                  {manualError}
                </div>
              )}
            </div>
          )}

          <div className="privacy-settings">
            <div className="privacy-settings-header">
              <div>
                <h3>Privacy Settings</h3>
                <p>Select which types of sensitive data Veil should detect.</p>
              </div>
            </div>

            <div className="privacy-categories">
              {PRIVACY_CATEGORIES.map((category) => (
                <label
                  key={category.value}
                  className="privacy-category"
                >
                  <input
                    type="checkbox"
                    disabled={loading}
                    checked={selectedEntities.includes(category.value)}
                    onChange={() => toggleEntity(category.value)}
                  />

                  <span>{category.label}</span>
                </label>
              ))}
            </div>
          </div>

          <div className="actions">
            <span className="engine-label">
              English · Presidio + spaCy
            </span>

            <button
              type="button"
              onClick={detectPII}
              disabled={
                loading ||
                !text.trim() ||
                selectedEntities.length === 0
              }
            >
              {loading ? "Detecting..." : "Detect Sensitive Data →"}
            </button>
          </div>

          {selectedEntities.length === 0 && (
            <div className="error">
              Select at least one sensitive data category.
            </div>
          )}

          {error && (
            <div className="error" role="alert">
              Detection failed: {error}
            </div>
          )}
        </section>

        {analyzed && (
          <section className="panel results-panel">
            <div className="panel-header">
              <div>
                <h3>Detection Results</h3>
                <p>
                  Review the sensitive information identified by Veil.
                </p>
              </div>

              <span className="result-count">
                {detections.length}{" "}
                {detections.length === 1 ? "entity" : "entities"}
              </span>
            </div>

            {reviewError && (
              <div className="error" role="alert">
                {reviewError}
              </div>
            )}

            {detections.length === 0 ? (
              <div className="empty-results">
                No sensitive information was detected for the selected
                categories. You can select text above to add missed PII.
              </div>
            ) : (
              <div className="detection-list">
                {detections.map((item) => (
                  <div className="detection-item" key={item.id}>
                    <div>
                      <strong>{item.text}</strong>

                      <select
                        aria-label={`Category for ${item.text}`}
                        value={item.entity_type}
                        disabled={loading}
                        onChange={(event) =>
                          updateDetectionCategory(
                            item.id,
                            event.target.value
                          )
                        }
                      >
                        {PRIVACY_CATEGORIES.map((category) => (
                          <option
                            key={category.value}
                            value={category.value}
                          >
                            {category.label}
                          </option>
                        ))}
                      </select>
                    </div>

                    <div className="review-controls">
                      <button
                        type="button"
                        disabled={loading || !manualSelection}
                        onClick={() => updateDetectionBoundary(item.id)}
                      >
                        Use selected text
                      </button>
                      <div className="confidence">
                        {item.confidence === null
                          ? "Manually reviewed"
                          : `${(item.confidence * 100).toFixed(0)}% score`}
                      </div>

                      <span className="review-status">
                        {item.status === "pending"
                          ? "Pending review"
                          : item.status === "accepted"
                            ? "Accepted"
                            : "Rejected"}
                      </span>

                      <button
                        type="button"
                        onClick={() =>
                          updateReviewStatus(item.id, "accepted")
                        }
                        disabled={loading || item.status === "accepted"}
                      >
                        Accept
                      </button>

                      <button
                        type="button"
                        onClick={() =>
                          updateReviewStatus(item.id, "rejected")
                        }
                        disabled={loading || item.status === "rejected"}
                      >
                        Reject
                      </button>

                      {item.status !== "pending" && (
                        <button
                          type="button"
                          disabled={loading}
                          onClick={() =>
                            updateReviewStatus(item.id, "pending")
                          }
                        >
                          Reset
                        </button>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>
        )}
      </main>

      <footer>Veil v0.1 · Local-first privacy engine</footer>
    </div>
  );
}

export default App;