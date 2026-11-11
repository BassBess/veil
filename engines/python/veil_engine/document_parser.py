"""Local-only document extraction. No detector or network dependencies are loaded here."""
from pathlib import Path
from zipfile import ZipFile, BadZipFile

MAX_FILE_BYTES = 20 * 1024 * 1024
MAX_TEXT_BYTES = 100_000
MAX_ARCHIVE_BYTES = 100 * 1024 * 1024
SUPPORTED_TYPES = {"txt", "docx", "pdf", "csv", "xlsx"}
PDF_WARNING = "This PDF may be scanned or image-based. OCR is not yet supported."


class DocumentError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def _join(parts):
    result = []
    size = 0
    for part in parts:
        if not part:
            continue
        size += len(part.encode("utf-8")) + (1 if result else 0)
        if size > MAX_TEXT_BYTES:
            raise DocumentError("text_too_large", "Extracted text exceeds the 100,000-byte limit. Use a smaller document.")
        result.append(part)
    return "\n".join(result)


def _read_text(path):
    try:
        text = path.read_bytes().decode("utf-8-sig")
    except UnicodeDecodeError:
        raise DocumentError("encoding", "This file is not valid UTF-8. Save it as UTF-8 and try again.") from None
    if "\x00" in text:
        raise DocumentError("invalid_file", "This file contains binary data. Choose a plain-text UTF-8 file.")
    return text


def _txt(path):
    return _read_text(path), None, []


def _docx(path):
    from docx import Document
    from docx.table import Table
    def blocks(container):
        for block in container.iter_inner_content():
            if isinstance(block, Table):
                for row in block.rows:
                    # Preserve nested paragraph/table order inside each cell.
                    yield " | ".join(_join(blocks(cell)) for cell in row.cells)
            else:
                yield block.text
    return _join(blocks(Document(path))), None, []


def _pdf(path):
    from pypdf import PdfReader
    reader = PdfReader(path)
    if reader.is_encrypted:
        raise DocumentError("encrypted", "Password-protected PDFs are not supported. Choose an unlocked copy.")
    if len(reader.pages) > 500:
        raise DocumentError("too_many_pages", "This PDF exceeds the 500-page limit. Use a smaller document.")
    sparse = False
    def pages():
        nonlocal sparse
        for page in reader.pages:
            content = page.extract_text() or ""
            if sum(char.isalnum() for char in content) < 10:
                sparse = True
            yield content
    text = _join(pages())
    return text, len(reader.pages), [PDF_WARNING] if sparse or not text.strip() else []


def _frame_rows(frame):
    for row in frame.itertuples(index=False, name=None):
        yield " | ".join(str(cell).replace("\r", " ").replace("\n", " ") for cell in row)


def _csv(path):
    import io
    import pandas as pd
    # Read all rows as strings: do not drop headers, leading zeros or literal NA.
    chunks = pd.read_csv(io.StringIO(_read_text(path)), header=None, dtype=str,
                         keep_default_na=False, chunksize=500)
    return _join(line for chunk in chunks for line in _frame_rows(chunk)), None, []


def _xlsx(path):
    import pandas as pd
    def sheets():
        with pd.ExcelFile(path, engine="openpyxl") as workbook:
            for name in workbook.sheet_names:
                yield f"Sheet: {name}"
                frame = pd.read_excel(workbook, sheet_name=name, header=None,
                                      dtype=str, keep_default_na=False)
                yield from _frame_rows(frame)
    return _join(sheets()), None, []


def _check_archive(path):
    with ZipFile(path) as archive:
        if sum(entry.file_size for entry in archive.infolist()) > MAX_ARCHIVE_BYTES:
            raise DocumentError("archive_too_large", "This document expands beyond the safe parsing limit. Use a smaller document.")
        if any(entry.flag_bits & 1 for entry in archive.infolist()):
            raise DocumentError("encrypted", "Encrypted documents are not supported. Choose an unlocked copy.")


def parse_document(file_path):
    path = Path(file_path)
    kind = path.suffix.lower().lstrip(".")
    if kind not in SUPPORTED_TYPES:
        raise DocumentError("unsupported_type", "Choose a TXT, DOCX, PDF, CSV, or XLSX document.")
    try:
        if not path.exists():
            raise DocumentError("missing_file", "The selected file no longer exists. Select it again.")
        if not path.is_file():
            raise DocumentError("invalid_file", "Select a regular document file.")
        size = path.stat().st_size
        if size > MAX_FILE_BYTES:
            raise DocumentError("file_too_large", "The file exceeds the 20 MB limit. Choose a smaller document.")
        if size == 0:
            raise DocumentError("empty_file", "The selected file is empty.")
        if kind in {"docx", "xlsx"}:
            _check_archive(path)
        text, pages, warnings = {"txt": _txt, "docx": _docx, "pdf": _pdf,
                                  "csv": _csv, "xlsx": _xlsx}[kind](path)
        # Textareas normalize line endings; keep stored text and review offsets aligned.
        text = _join([text.replace("\r\n", "\n").replace("\r", "\n")])
        if not text.strip() and kind != "pdf":
            raise DocumentError("empty_document", "No readable text was found in this document.")
        return {"text": text, "file_name": path.name, "file_type": kind,
                "pages": pages, "warnings": warnings}
    except DocumentError:
        raise
    except PermissionError:
        raise DocumentError("unreadable_file", "Veil cannot read this file. Check its permissions or select another copy.") from None
    except ImportError:
        raise DocumentError("parser_unavailable", "Document parsing dependencies are missing. Install the Python requirements and restart Veil.") from None
    except (OSError, BadZipFile):
        raise DocumentError("invalid_file", "The document could not be read. It may be damaged, locked, or in the wrong format.") from None
    except Exception:
        # Never expose document contents or local paths in parser errors.
        raise DocumentError("invalid_file", "The document could not be parsed. It may be damaged, encrypted, or in the wrong format.") from None
