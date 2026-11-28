# Veil modular privacy engine

## Using the desktop engine

Open **Model Library** to import a local file, a directory, or a `.veilmodel` archive. For a public Hugging Face repository, enter a repository ID, model URL, revision URL, or weight file URL and choose **Download and install**. The library displays each version, license, size, SHA-256 inventory, available adapters, and current load state. **Load and validate** probes the local runtime. **Unload** releases the library's session reference. An active profile loads its models again on the next detection.

For an older Hub installation with omitted files, choose **Repair from Hugging Face**. Veil checks the local files against their recorded digests and fetches only missing or changed files from the original pinned revision. Repair keeps the library ID, existing label decisions, and profile references. Switch away from a profile that uses the model before repairing or changing its labels.

Choose **Configure labels** to inspect automatic aliases, previously approved mappings, and suggested custom codes. **Apply all suggested mappings** saves the suggestions across the model's adapters in one step and keeps manual choices. You can select a different Veil category, enter a custom code, or explicitly ignore a native label group. Saving unresolved choices keeps an editable draft. Activation requires every group needed by the selected adapter to be resolved, at least one sensitive category, the required files, and a successful runtime probe. Custom categories appear in **Anonymize** with document-local tokens such as `CUSTOM_USERNAME_001`. The Vault workflow uses the built-in categories.

**Reset to recommended mappings** previews a fresh mapping from the original labels and the shared taxonomy. The preview shows the current and recommended value for each changed label. Confirming replaces those values, including custom assignments or explicit ignores, and assigns stable sensitive categories to unfamiliar labels. Reset leaves the installed files, model ID, and saved profile references in place. The library requires original label metadata for this action.

To use ONNX, install the optional runtime into the existing Python environment.

```sh
cd engines/python
.venv/bin/python -m pip install -r requirements-onnx.txt
```

Use `.venv\Scripts\python.exe` on Windows. Runtime dependencies are installed separately from model artifacts.

Open **Profiles** to create, edit, duplicate, import, export, delete, or activate a profile. Choose the languages, legacy NLP setting, task-specific components, and resource limits. **Use existing detection** clears the active profile. Switching profiles or editing the active profile clears the current document's detection review so that a subsequent run uses the new configuration. Review and apply detections in the document workspace as before.

Veil requires the complete detection request to succeed before it exposes suggestions. On cancellation, a budget failure, incompatible input, or a component failure, rerun detection after resolving the error. The Profiles view shows content-free metrics for the last request in the current engine process.

## Architecture and privacy

- **Registry and data contracts.** Components register capabilities with typed handlers and may implement multiple capabilities. A span identifies a Veil category, confidence, source text, and half-open Unicode code-point offsets. Contextual generators produce ranges, and classifiers return sensitivity spans for those ranges. A native mapping interface connects reviewed entities to the existing document-local or encrypted Vault mapper, as selected by the processing mode.
- **Shared core.** Every profile runs the original inline rules and selected-category structured recognizers, including normalization and checksums. The core can use the existing spaCy and Stanza models. Clearing the legacy NLP option uses local tokenization with the deterministic recognizers. Existing Vault phrases, scoped exclusions, core ranking, and original-text validation continue to apply. Core suggestions keep their precedence when extra model spans overlap them. Rust still validates reviewed ranges and owns replacement, Vault transactions, and export.
- **Profiles.** Each profile is a versioned JSON recipe. Steps execute in detection, candidate generation, classification, fusion, and ranking order. The profile must contain one fusion and ranking step, use compatible language sets, and pair contextual classification with candidate generation. Activation validates the required legacy data and loads and probes the referenced model adapters. Missing dependencies in an inactive profile can be repaired in the editor. Switch away from an active profile, or edit its dependencies, before removing a model it requires.
- **Static controller.** All configured steps run with their declared inputs, including when the deterministic core finds matches. The routing interface currently uses a static policy. It checks elapsed time, process resident memory where available, and component and native model call counts at execution boundaries. It records latency, memory samples, counts, failures, and routing decisions by component. The native worker also has a 180-second watchdog. The service keeps the latest metrics in memory and discards the request and response objects after replying.
- **Local storage.** `privacy-engine/engine.sqlite3` under Tauri's app-data directory contains the model and profile catalogs and active profile ID. An installation's directory ID is initially derived from its metadata, adapter configuration, and file hashes. Repair and reconfiguration keep that ID while updating the catalog. Profiles share installed files and loaded runtime sessions. A staged import or repair is published under a transaction after validation and hashing. Download receipts reuse a cached selection from a pinned revision. Application profiles and models are independent of the encrypted Vault and contain configuration and model data.
- **Download boundary.** The desktop runs local inference and downloads in separate processes. The download request contains a repository locator and cancellation marker. It resolves the commit, then makes anonymous HTTPS GET requests to the Hub and checks every redirect against exact Hugging Face download hostnames on port 443. The importer validates published LFS SHA-256 digests and manifest hashes before installation. The inference process blocks Python networking, disables ONNX Runtime telemetry before initialization, uses the CPU provider, and loads tokenizer configuration and verified model bytes locally. Imported repository scripts, executable model formats, custom ONNX operators, and external tensor files have no execution path. [Hugging Face download endpoints](https://huggingface.co/docs/hub/en/models-downloading#downloading-behind-a-proxy-or-firewall) · [Hugging Face endpoint metadata](https://huggingface.co/.well-known/meta.json) · [ONNX Runtime privacy documentation](https://github.com/microsoft/onnxruntime/blob/main/docs/Privacy.md)

On first use with an existing Veil installation, the model catalog and custom profiles start empty. The existing detection workflow is selected by default and continues to use its installed language models. Vault data, categories, and settings remain in their existing storage. Exported profiles contain component settings and artifact IDs. To reuse a profile on another machine, install the same model bundles there before importing the profile.

## Model packages and ONNX adapters

A schema 1 model package is a directory with `veil-model.json` at its root. A `.veilmodel` archive is a ZIP with that same manifest and exactly the files listed in it. A Hugging Face repository can publish the same manifest layout. Use relative, portable paths and SHA-256 hashes of the exact file bytes. A bundle may declare one adapter per capability. The following example uses placeholder hashes.

```json
{
  "schema": 1,
  "name": "Synthetic token classifier",
  "version": "1.0",
  "license": "CC0-1.0",
  "files": {
    "model.onnx": "0000000000000000000000000000000000000000000000000000000000000000",
    "tokenizer.json": "1111111111111111111111111111111111111111111111111111111111111111"
  },
  "adapters": [
    {
      "adapter": "onnx-token-v1",
      "model": "model.onnx",
      "tokenizer": "tokenizer.json",
      "languages": ["en"],
      "inputs": {"input_ids": "input_ids", "attention_mask": "attention_mask"},
      "output": "logits",
      "labels": ["O", "B-PERSON", "I-PERSON"],
      "max_length": 512,
      "stride": 64,
      "min_confidence": 0.65
    }
  ]
}
```

| Adapter | Native input and result |
| --- | --- |
| `onnx-token-v1` | Encodes document text with the bundled tokenizer and overlapping token windows. Reads `[1, tokens, labels]` floating-point logits. Decodes BIO and BIOES/BILOU tags and keeps original-text offsets. |
| `onnx-context-v1` | Takes a generated range. Encodes either the candidate and its surrounding text as a pair (`context_input: "pair"`) or the candidate alone (`"candidate"`). `context_chars` controls the maximum surrounding characters per side. Reads `[1, labels]` floating-point logits and returns the original candidate range. Uses category labels without BIO prefixes. |

Both adapters take int32/int64 inputs with dynamic token axes and batch size one. Map each required graph input with `inputs`. The currently supported roles are `input_ids`, `attention_mask`, and `token_type_ids`. The named output must match the adapter's expected rank and label count. Labels follow exact native index order. `O` marks the native background class. An optional `label_map` associates an uppercase native entity group with an existing Veil code, a `CUSTOM_` code, or `@ignore`. BIO and BIOES/BILOU boundaries stay attached to their original output indices. For example, `{"POSTCODE":"POSTAL_CODE","USERNAME":"CUSTOM_USERNAME","AGE":"@ignore"}` resolves three native groups. Veil automatically recognizes common aliases such as `PER`, `LOC`, `ORG`, `POSTCODE`, and `DATE_OF_BIRTH`. Scores are softmax outputs used as ranking signals. The adapter probes the graph with fictional text before it can run on a document. [Tokenizers Encoding documentation](https://huggingface.co/docs/tokenizers/main/api/encoding) · [ONNX Runtime Python API](https://onnxruntime.ai/docs/api/python/api_summary)

Raw ONNX imports can infer a token adapter from a token-classification architecture, an ordered `id2label` map, and supported languages. Hugging Face model-card language metadata can also supply the language declaration. A Veil manifest can specify other supported contracts. The library can display an adapter draft before the tokenizer is present or the label mappings are resolved.

Hugging Face BPE and Unigram tokenizer JSON can use up to 64 MiB. WordPiece and WordLevel retain a 32 MiB limit. Larger tokenizer artifacts must declare a recognized format and pass the structural checks before installation. The runtime validates every tokenizer before loading it. Vocabulary and BPE merge tables are capped at one million entries each. Other JSON metadata keeps its 2 MiB limit, and model file and bundle limits still apply.

### Shared entity taxonomy

`entity_taxonomy.py` gives equivalent entity names a stable Veil code. Lookups normalize Unicode, case, separators, and camel case. For example, `CVV`, `creditCardCvv`, and `card security code` all resolve to `CUSTOM_CREDIT_CARD_CVV`. This code is distinct from `CREDIT_CARD`. The taxonomy also distinguishes dates from dates of birth, API keys from passwords and generic secrets, and passports from national IDs and Social Security numbers.

New imports use recognized aliases and unambiguous positive approvals from the local library. Confirmed custom mappings can be reused across models with the same normalized label. If previous approvals disagree, Veil asks for an explicit local choice. Ignores stay with their model. Opaque labels such as `LABEL_1` get model-scoped suggestions because their meaning depends on the model. Other unfamiliar labels get stable custom suggestions and stay unresolved until confirmed.

The schema update adds an approval table to the local model catalog. Model IDs, applied runtime labels, profile references, and the active profile are preserved on upgrade. Approval lookup uses the local catalog and bundled taxonomy. To extend the taxonomy, add equivalent spellings to an existing entry or add a separate category where the meaning differs, then test both equivalence and distinctions.

### Adding an adapter

1. Choose the capability in `components.py` and define the native contract in `model_specs.py`. Add a versioned adapter identifier. Validate the file roles, label vocabulary, thresholds, language support, and tensor or feature layout.
2. Implement the capability handler and add its factory in `runtime.py`. Introduce another loader through the runtime interface when a format requires it. Use local, hash-verified data and an installed runtime. Keep architecture, tokenizer, and inference controls declarative.
3. Validate input names, dtypes, dimensions, output classes, and offsets. Probe on fictional input at load. Return standard spans from the adapter and call the controller at inference boundaries.
4. Add an adapter test with a small synthetic model and fixture package. Exercise original Unicode offsets, inputs the model rejects, missing dependencies, cancellation or budget exhaustion, offline operation, and activation from a saved profile.

## Scope and limitations

Phase 1 includes CPU ONNX adapters for the contracts above. Other runtime formats, private and gated Hugging Face repositories, ONNX external weights, and custom operators require additional adapters or import support. Individual files are limited to 512 MiB, and bundles to 1 GiB and 32 data files. Downloads can be cancelled and retried from the beginning. A killed process can leave an unreferenced staging directory, which remains outside the installed catalog.

Memory limits measure the whole worker process, including loaded models. macOS reports peak resident memory. Budgets and cancellation are checked between operations, so a native call may run until it returns or the watchdog stops its worker. Download progress can be indeterminate when Hugging Face omits sizes. The desktop retains its repository-relative Python environment setup.

The built-in contextual candidate generator recognizes capitalized phrases in English, French, or Spanish. It is a heuristic and requires a classifier. Model probes establish runtime compatibility. Detection quality still depends on the model, tokenizer, and profile, and Veil still requires document review.

Automatic file repair requires a pinned Hub source. Label decisions are local installation settings. Exported profiles refer to installed model IDs, so a fresh install on another machine can have a different ID from an older installation repaired in place. The same label choices can be declared in a portable Veil model manifest.

## Modified files

| Area | Files |
| --- | --- |
| Registry, detection and scheduling | `engines/python/veil_engine/{catalog,components,controller,pipeline,detector,languages,bridge}.py` |
| Model lifecycle and profile persistence | `engines/python/veil_engine/{model_store,model_import,model_download,model_configuration,profiles,service}.py` |
| Label resolution and adapter validation | `engines/python/veil_engine/{entity_taxonomy,labels,model_specs,runtime}.py`, `engines/python/requirements-onnx.txt` |
| Desktop navigation and engine controls | `apps/desktop/src/{App.tsx,App.css,EnginePanel.tsx,ModelLabelEditor.tsx}` |
| Native IPC and workflow | `apps/desktop/src-tauri/src/{engine,lib,protection,review,documents}.rs`, `apps/desktop/src-tauri/src/database/tests.rs` |
| Fixtures and documentation | `engines/python/tests/{test_modular_engine,test_onnx_adapters,test_model_download,test_model_reconfiguration,test_entity_taxonomy}.py`, `docs/modular-privacy-engine.md`, `README.md` |
| Linux development shells | `flake.nix` |

## Tests

Run the following from the repository root in the existing development environment.

```sh
(cd engines/python && .venv/bin/python -m unittest discover -s tests -v)
(cd apps/desktop && node --test tests/*.test.mjs && npm run lint && npm run build)
(cd apps/desktop/src-tauri && cargo test --locked)
(cd apps/desktop/src-tauri && cargo test --locked -- --ignored --skip compare_legacy_and_sqlcipher)
```

The engine tests cover multiple registered capabilities, incompatible profiles, shared artifacts, pinned downloads, cancellation and staging cleanup, hash and archive validation, atomic activation, profile import and export, deterministic behavior without legacy NLP, suppression of partial results, content-free diagnostics, and persistence across process restarts. The optional real ONNX tests generate their own token classifier and contextual classifier graphs and exercise offline loading, input layout validation, Unicode offsets, shared sessions, and forbidden ONNX features. The Rust IPC test runs profile activation and inference through the desktop's actual worker.

### Phase 1 baseline verification on Linux

| Check | Result |
| --- | --- |
| Python with optional ONNX dependencies | 84 passed. Two legacy Arabic checks require the local Stanza data and had the same errors on the baseline. All 17 Phase 1 checks passed. |
| Default Rust suite | 66 passed and 9 ignored. |
| Optional native detection and import suite | 7 passed, including the new IPC test. The remaining legacy multilingual check requires the same Stanza data. The timing benchmark was excluded. |
| Frontend | 20 tests passed. ESLint, TypeScript, and Vite passed. |

The transport regression tests use a mocked Hub. The optional ONNX suite uses locally generated fixtures. Native verification ran on Linux through the worker and headless test entry points.

### Live Horizon repair and inference

The Hub resolved the quantized Horizon Small model to revision `a20835db9e671267b21a2951f5bf2c76e4901708`. Both the branch and pinned weight URLs returned an HTTP 302 from `huggingface.co` to `us.aws.cdn.hf.co`. Veil downloaded the pinned 268,496,006-byte ONNX file and matched the Hub's LFS SHA-256 digest `0866ce86f352b1a1149049e87511aa68b1cb87958a3a4fa5ef5f63536d94ffa0`. [Horizon Small model card](https://huggingface.co/Horizon-Labs/pii-redactor-small)

The test started with an isolated library produced by the older importer. Repair fetched the 34,363,288-byte tokenizer and verified its published SHA-256. The original weights kept their digest, inode, and modification time. The BPE tokenizer has 256,000 vocabulary entries, 580,604 merges, and two special tokens. The `ModernBertForTokenClassification` graph has ONNX IR 8, opset 17, int64 `input_ids` and `attention_mask` with dynamic batch and sequence axes, and a float32 logits axis with 59 classes. Of the 29 native entity groups, 12 resolved automatically and the test explicitly mapped the other 17 to custom categories.

The real model activated and ran in Veil's offline IPC worker. These are the model-produced spans from four fictional documents. Offsets are half-open Unicode code-point indices. Each document also produced one shared-recognizer email span.

| Language | Model spans and offsets |
| --- | --- |
| English | Maya Chen `[2,11)`, Boston `[21,27)`, orchid_moon `[74,85)` as a custom username, and 483 `[97,100)` as a custom CVV |
| French | Élodie Martin `[2,15)` |
| Spanish | García López `[2,14)`, Bogotá `[23,29)` |
| Arabic | سارة الخطيب `[8,19)`, بيروت `[23,28)` |

Every returned span matched its source slice, and the final suggestions were non-overlapping. A five-invocation budget and a 1 MiB memory budget each failed without exposing partial suggestions. The saved profile survived a worker restart and produced five English suggestions after the model reloaded. The Arabic fixture's worker reported one native model call at about 26 ms and about 1.24 GiB of process RSS. These figures describe one local run.

The new regression tests cover alias and ignore decisions, the per-format limits, Unicode token offsets, repair integrity and cancellation, active-profile protection, restart persistence, and native document-local custom tokens. The follow-up passed 95 Python checks, 67 default Rust tests, 7 optional Rust tests, and 20 frontend tests. Two Python checks and one optional Rust check still require the legacy Arabic Stanza data. ESLint and the production frontend build passed.

### Live Horizon and Nym taxonomy check

The offline desktop worker ran Horizon Small at `a20835db9e671267b21a2951f5bf2c76e4901708` and Nym Small's FP32 ONNX at `4348999cd3c2e20c49615e9af7c6bbb45b64cd85` in an isolated library. The catalog upgrade preserved the existing model and profile rows. The suggestions covered all 29 Horizon and 40 Nym entity groups. Fourteen shared label pairs resolved to the same Veil code. [Horizon Small model card](https://huggingface.co/Horizon-Labs/pii-redactor-small) · [Nym Small model card](https://huggingface.co/Wismut/nym-pii-multilingual-small)

In the English fixture, both models separately detected the fictional username `orchid_moon`, CVV `483`, and passport `XZ1234567` with the same category codes. The combined profile returned one suggestion for each. Both models made native calls for each combined English, French, Spanish, and Arabic fixture. Returned offsets matched the original Unicode slices, and the profile stayed active after restarting the worker.

All seven new taxonomy regressions passed. The full Python run passed 102 checks, with the same two checks requiring legacy Arabic Stanza data. The native profile IPC check, 20 frontend tests, ESLint, and the production build passed.

### Reset check on existing installations

An isolated copy of the Horizon and Nym library was given representative legacy label choices. The reset preview showed four changes for Horizon and five for Nym. Confirmation restored shared CVV, driver license, and postal code mappings and kept birth date, payment card number, and PIN distinct. Every artifact SHA-256, both library IDs, and all saved profile definitions matched the pre-reset state. The same combined profile ran both models after reactivation and stayed active after restarting the worker.
