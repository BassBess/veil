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

function App() {
  const [text, setText] = useState(
    "John Smith lives in New York. Contact him at john.smith@example.com."
  );

  const [detections, setDetections] = useState<Detection[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [analyzed, setAnalyzed] = useState(false);

  async function detectPII() {
    if (!text.trim()) return;

    setLoading(true);
    setError("");
    setDetections([]);
    setAnalyzed(false);

    try {
      const response = await invoke<DetectionResponse>("detect_pii", {
        text,
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
            <span>Privacy starts here.</span>
          </div>
        </div>

        <div className="status">● Local Processing</div>
      </header>

      <main className="main">
        <div className="intro">
          <span className="eyebrow">PRIVACY WORKSPACE</span>
          <h2>Protect what matters.</h2>
          <p>
            Detect sensitive information in your text
            before sharing it with others.
          </p>
        </div>

        <section className="panel">
          <div className="panel-heading">
            <h3>Document Input</h3>
            <span>{text.length} characters</span>
          </div>

          <textarea
            value={text}
            onChange={(event) => {
              setText(event.target.value);
              setDetections([]);
              setAnalyzed(false);
              setError("");
            }}
            placeholder="Paste your text here..."
            maxLength={100000}
          />

          <div className="actions">
            <span>English · Presidio + spaCy</span>

            <button
              onClick={detectPII}
              disabled={loading || !text.trim()}
            >
              {loading ? "Analyzing..." : "Detect Sensitive Data →"}
            </button>
          </div>
        </section>

        {error && (
          <div className="error">
            Detection failed: {error}
          </div>
        )}

        {analyzed && (
          <section className="panel results">
            <div className="panel-heading">
              <h3>Detection Results</h3>
              <span>{detections.length} entities found</span>
            </div>

            {detections.length === 0 ? (
              <p>No sensitive information detected.</p>
            ) : (
              <div className="detection-list">
                {detections.map((item, index) => (
                  <div className="detection" key={index}>
                    <div>
                      <strong>{item.text}</strong>
                      <span className="category">
                        {item.entity_type}
                      </span>
                    </div>

                    <span className="confidence">
                      {(item.confidence * 100).toFixed(0)}% score
                    </span>
                  </div>
                ))}
              </div>
            )}
          </section>
        )}

        <footer>
          Veil v0.1 · Local-first privacy engine
        </footer>
      </main>
    </div>
  );
}

export default App;
