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

type DetectionResponse = {
  detections: Detection[];
};

type PrivacyCategory = {
  label: string;
  value: string;
};

const PRIVACY_CATEGORIES: PrivacyCategory[] = [
  { label: "Person names", value: "PERSON" },
  { label: "Email addresses", value: "EMAIL_ADDRESS" },
  { label: "Phone numbers", value: "PHONE_NUMBER" },
  { label: "Locations", value: "LOCATION" },
  { label: "Organizations", value: "ORGANIZATION" },
];

function App() {
  const [text, setText] = useState(
    "John Smith lives in New York. Contact him at john.smith@example.com."
  );

  const [detections, setDetections] = useState<Detection[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [analyzed, setAnalyzed] = useState(false);

  const [selectedEntities, setSelectedEntities] = useState<string[]>(
    PRIVACY_CATEGORIES.map((category) => category.value)
  );

  function toggleEntity(entity: string) {
    setSelectedEntities((current) => {
      if (current.includes(entity)) {
        return current.filter((item) => item !== entity);
      }

      return [...current, entity];
    });
  }

  async function detectPII() {
    if (!text.trim()) {
      return;
    }

    setLoading(true);
    setError("");
    setDetections([]);
    setAnalyzed(false);

    try {
      const response = await invoke<DetectionResponse>("detect_pii", {
        text,
        entities: selectedEntities,
      });

      setDetections(response.detections);
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
            value={text}
            onChange={(event) => setText(event.target.value)}
            maxLength={100000}
            placeholder="Paste your text here..."
          />

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
              onClick={detectPII}
              disabled={
                loading ||
                !text.trim() ||
                selectedEntities.length === 0
              }
            >
              {loading
                ? "Detecting..."
                : "Detect Sensitive Data →"}
            </button>
          </div>

          {selectedEntities.length === 0 && (
            <div className="error">
              Select at least one sensitive data category.
            </div>
          )}

          {error && (
            <div className="error">
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
                {detections.length === 1
                  ? "entity"
                  : "entities"}
              </span>
            </div>

            {detections.length === 0 ? (
              <div className="empty-results">
                No sensitive information was detected for the selected
                categories.
              </div>
            ) : (
              <div className="detection-list">
                {detections.map((item, index) => (
                  <div
                    className="detection-item"
                    key={`${item.start}-${item.end}-${index}`}
                  >
                    <div>
                      <strong>{item.text}</strong>
                      <span>{item.entity_type}</span>
                    </div>

                    <div className="confidence">
                      {(item.confidence * 100).toFixed(0)}% score
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>
        )}
      </main>

      <footer>
        Veil v0.1 · Local-first privacy engine
      </footer>
    </div>
  );
}

export default App;