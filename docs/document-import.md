# Local document import

Veil accepts TXT, DOCX, PDF, CSV and XLSX files through **Upload document** beside
**Paste text**. The picker is the operating system's native dialog. Parsing and
PII detection run on the device; documents are never uploaded and source files
are never modified.

## Setup and run

Use the project's existing Python virtual environment (Python 3.12+) and install
the updated requirements. These dependencies and the spaCy model must be installed
before running offline. Dependency installation itself needs package access or
an offline wheel cache.

macOS, from the repository root:

```sh
engines/python/.venv/bin/python -m pip install -r engines/python/requirements.txt
cd apps/desktop
npx tauri dev
```

Windows PowerShell, from the repository root:

```powershell
engines/python/.venv/Scripts/python.exe -m pip install -r engines/python/requirements.txt
cd apps/desktop
npx tauri dev
```

The Rust dialog plugin is installed through Cargo. No JavaScript dialog or
filesystem dependency is needed: the frontend invokes `parse_document`, and Rust
owns file selection and passes the selected path directly to Python. The command
accepts no path from JavaScript. No broad filesystem capability is added.

The existing engine setup resolves `engines/python/.venv` relative to the source
tree. Standalone distribution still needs the project's Python/model packaging
solution; this feature does not bundle a Python runtime.

## Behavior

1. Choose **Upload document** and select a supported local file.
2. Veil shows a loading state, then the file name, type, PDF page count when
   applicable, extracted text, and any warnings.
3. Select categories and use the existing Detect / review / correction / Continue
   flow. No detection runs automatically during import.
4. **Replace file** opens the picker again. Cancellation and parsing failures keep
   the current document and review intact. Successful replacement starts a new
   review with the imported text.
5. **Remove file** clears the imported text, file metadata and review. **Paste
   text** detaches file metadata and focuses the textarea, retaining its current
   text for convenience.
6. Editing imported text clears detections and validation, as requested for this
   feature. Pasted-text edits retain the previously implemented review-preserving
   behavior. Boundary corrections through Edit / Save correction continue to work
   for both input methods.

## Extraction and limits

- TXT and CSV require UTF-8, with an optional BOM. Encoding errors are reported
  rather than silently replacing characters. Binary/NUL-containing input is rejected.
- DOCX extracts body paragraphs and tables in their document order, including
  nested table contents. Headers, footers, comments and text inside images are
  not extracted.
- PDF extracts page by page, separated by newlines. Pages with fewer than ten
  alphanumeric characters trigger a possible scanned/image-based PDF warning.
  Empty/scanned PDFs may return empty text plus that warning; **OCR is not
  implemented**. Text layout, especially multiple columns, depends on the PDF's
  underlying text ordering. Password-protected PDFs are rejected.
- CSV uses comma-separated rows and ` | ` between cells, preserving the first
  row, leading zeros stored as text, and literal `NA` values.
- XLSX includes sheet names and rows in workbook order. Formulas use cached
  values; formulas are never executed or recalculated. Numeric display formatting,
  charts and images are not preserved. Password-encrypted workbooks are unsupported.
- Files: configurable in Settings (20, 50 or 100 MiB; default 100 MiB). Extracted text: maximum 500,000 UTF-8 bytes. Oversized extraction fails without truncating text.
- DOCX/XLSX archives: maximum 100 MiB declared uncompressed size. PDFs: maximum
  500 pages. Python parsing is terminated by Rust after 90 seconds.
- Review state remains in memory, matching the existing app; import adds no
  document storage, cloud service or external API.

Email validation uses tldextract's packaged suffix snapshot, with network updates
and disk caching disabled. Detection fails if its spaCy model is absent rather
than attempting an automatic download at runtime.

## Verification

From `engines/python`:

```sh
.venv/bin/python -m unittest discover -s tests -v
```

On Windows, replace `.venv/bin/python` with `.venv/Scripts/python.exe`.

From the repository root:

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml -- --include-ignored
node --test apps/desktop/tests/rebaseDetections.test.mjs
npm run build --prefix apps/desktop
npm run lint --prefix apps/desktop
```

The two Rust integration tests normally marked ignored require the Python venv;
`--include-ignored` runs them along with the regular unit tests. Parser tests
create fixtures locally for all five formats, malformed/empty/oversized files,
encoding, permissions, Unicode paths, encrypted PDFs, archive expansion and sparse
PDFs. Python bridge integration denies socket connections and verifies imported
text reaches the existing email detector. Rust integration additionally verifies
Rust → Python extraction → existing detection and structured parsing errors.

Manual desktop smoke check (native dialogs require a running desktop window):

- Import each supported format; confirm metadata, extracted text and warnings.
- Cancel replacement and try a damaged file; confirm the previous review remains.
- Edit imported text; confirm stale detections clear, then detect again.
- Accept/reject, correct a range, and Continue to backend validation.
- Remove the file and test pasted-text detection again.
- Import a scanned PDF; confirm the warning and disabled Detect for empty text.
- Check native picker behavior on both macOS and Windows.

## Bulk review and large text

- Filter results by category, search text, or review status, then use Accept all / Reject all. These actions include every matching result across the paginated list.
- Individual decisions link pending matches by text and category by default. Explicit per-occurrence rejections are preserved. Accepting an item also includes non-overlapping exact occurrences missed by detection; turn off linking for a single-occurrence decision.
- Process accepted items alone, or use Anonymize/Pseudonymize all detections to include pending items. Explicit rejections are always excluded. Stale or overlapping selections must still be corrected or rejected.
- Text extraction rejects oversized content without truncation. Detection processes long text in 20,000-character windows with 1,024-character overlap and globally resolves overlapping results.
- File size and extracted-text limits are independent. A large image-heavy PDF can contain little extractable text; OCR and preservation of the original PDF layout remain separate features.
