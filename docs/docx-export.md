# Protected DOCX export

FYP-17 adds Export DOCX to processed results from an imported Word document. Both anonymization and pseudonymization use the exact replacement tokens generated during processing. The export modifies a snapshot of the imported DOCX rather than rebuilding a document from plain text.

## Workflow

1. Upload a DOCX in the Anonymize or Pseudonymize workspace.
2. Detect and review the information. Add missed detections or correct categories and boundaries as needed.
3. Process accepted detections, or use the existing all-detections action.
4. Select Export DOCX and save the output.
5. Open the output in Word and review it before sharing.

Adding or editing a detection does not change the source text. Editing the imported text itself invalidates formatted export. Reimport and process the original document to enable it again. Cancelled or failed imports do not replace a successful source snapshot.

The original DOCX snapshot and export mappings are available only in the current workspace session. Export DOCX is not available from saved Vault history. The source snapshot is not added to Vault storage. Closing the app releases it. The export also works if the original file is removed after importing it.

## Preserved content

Supported replacements cover normal paragraphs, table cells including nested tables, and available header and footer variants. Import and export use the same extraction order and Unicode character positions. Only selected occurrences change, including repeated values.

Run styles, font settings, paragraph styles, tables, images, explicit page breaks, and other unmodified package members are retained. If a selection spans several differently formatted text runs, the replacement inherits the formatting at the start of the selection. Surrounding text retains its own formatting. Fonts still depend on the software and fonts installed on the recipient's computer. Replacement length can change wrapping and pagination; fixed page geometry is not guaranteed.

Links containing replaced text are converted to ordinary styled runs. The hyperlink wrapper, including its tooltip, is removed. Its relationship is removed when no other content in that part references it. Unselected links remain, including a destination intentionally retained by another unselected occurrence.

## Scope and limitations

This feature replaces reviewed text; it is not a full DOCX sanitization tool. Rejected and undetected information remains. Images, comments, tracked changes, text boxes, footnotes, endnotes, field instructions, metadata, custom XML, and embedded files are outside the reviewed text mapping and can retain sensitive information. Review or remove that content separately before sharing. Images are preserved rather than analyzed with OCR.

Selections crossing paragraph boundaries, table-cell separators, tabs, or line breaks are rejected instead of changing document structure. Review smaller selections. A merged table cell can appear multiple times in the extracted text while referring to the same underlying XML; those appearances must receive consistent replacement decisions.

Unsupported or malformed Word structures can fail import or export. DOC, DOCM, and encrypted Word files are not supported by this feature. PDF export remains a separate text-layout export; preserving an original PDF's layout is FYP-18.

## Validation and implementation

- Native imports retain bounded original bytes and exact extracted text behind a source identifier.
- Processing returns transient start, end, original-text, and replacement-token mappings. These mappings do not change the serialized Vault record format.
- Rust checks mappings, Unicode positions, overlap, source identity, and exact reconstruction of the processed preview.
- Python checks source text, bounds, overlap, editable text-node coverage, and merged-cell consistency before modifying XML.
- Python export runs locally without loading detection models or downloading resources.
- Native output uses the save dialog, a temporary output file, and an atomic persist after a successful export. Python rendering has a 90-second execution timeout after sending the request.
- Existing configurable import limits and the 500,000 UTF-8-byte extracted-text limit remain applicable. Expanded DOCX archives are bounded separately.

## Tests

Run from engines/python:

```powershell
.\.venv\Scripts\python.exe -m unittest discover -s tests -p test_docx_export.py -v
```

The 12 DOCX tests cover split styles, exact repeated occurrences, Unicode, headers/tables/footers, adjacent replacements, invalid and stale requests, structural boundaries, merged cells, offline bridge export, structured errors, and hyperlink destinations and tooltips.

Run from apps/desktop:

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml
cargo test --locked --manifest-path src-tauri/Cargo.toml original_snapshot_exports_after_source_file_is_removed -- --ignored
npm run build
npm run lint
```

Native tests cover exact Unicode output reconstruction, invalid mappings, source-cache cleanup, transient processing mappings in both modes, and exporting from the original snapshot after deleting the source file. The snapshot integration test requires the local Python environment and python-docx dependency.

Manual checks passed for basic font preservation, headers and footers, tables and images, both processing modes, selecting only the second repeated name, a name split across differently styled runs, edited-source export blocking, Arabic replacements including a manually added name, and three-page output with distinct local font settings and a retained table.

Detection observations for FYP-16: the Arabic example missed the name فاطمة and classified الجامعة الأمريكية as a location. The English sample also produced false positives for font names and the words Remain and Remains. These were corrected or rejected during review; detector changes are outside FYP-17.
