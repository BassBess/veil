"""Apply reviewed replacements to a DOCX without rebuilding its formatting."""
from bisect import bisect_left, bisect_right
from dataclasses import dataclass
from io import BytesIO
import posixpath
from zipfile import ZipFile

from docx import Document
from lxml import etree
from docx.oxml.ns import qn
from docx.table import Table

from veil_engine.document_parser import (
    DocumentError, MAX_FILE_BYTES, MAX_TEXT_BYTES, _check_archive,
)


def _normalize(value):
    return value.replace("\r\n", "\n").replace("\r", "\n")


@dataclass
class TextNode:
    start: int
    end: int
    element: object
    part: object


@dataclass
class DocumentText:
    text: str
    nodes: list


def _join_mapped(items, separator="\n", keep_empty=False):
    parts = []
    nodes = []
    length = 0
    for item in items:
        if not item.text and not keep_empty:
            continue
        if parts:
            length += len(separator)
        parts.append(item.text)
        nodes.extend(TextNode(node.start + length, node.end + length,
                              node.element, node.part) for node in item.nodes)
        length += len(item.text)
    return DocumentText(separator.join(parts), nodes)


def _paragraph_text(paragraph):
    nodes = []
    parts = []
    length = 0
    # Paragraph.text includes runs and hyperlink runs, but excludes text boxes.
    for content in paragraph._p.inner_content_elements:
        runs = [content] if content.tag == qn("w:r") else content.r_lst
        for run in runs:
            for element in run.xpath("w:br | w:cr | w:noBreakHyphen | w:ptab | w:t | w:tab"):
                value = _normalize(str(element))
                if element.tag == qn("w:t"):
                    nodes.append(TextNode(length, length + len(value), element,
                                          paragraph.part))
                parts.append(value)
                length += len(value)
    text = "".join(parts)
    if text != _normalize(paragraph.text):
        raise DocumentError("unsupported_docx", "This Word paragraph cannot be mapped safely for export.")
    return DocumentText(text, nodes)


def _blocks(container):
    for block in container.iter_inner_content():
        if isinstance(block, Table):
            for row in block.rows:
                yield _join_mapped((_join_mapped(_blocks(cell)) for cell in row.cells), " | ", keep_empty=True)
        else:
            yield _paragraph_text(block)


def extract_docx(document):
    """Keep the importer's header/body/footer order and synthetic separators."""
    def sections(kind):
        seen = set()
        for section in document.sections:
            for variant in ("", "first_page_", "even_page_"):
                container = getattr(section, variant + kind)
                if container.is_linked_to_previous:
                    continue
                name = str(container.part.partname)
                if name in seen:
                    continue
                seen.add(name)
                yield from _blocks(container)

    result = _join_mapped([
        *sections("header"), *_blocks(document), *sections("footer"),
    ])
    if len(result.text.encode("utf-8")) > MAX_TEXT_BYTES:
        raise DocumentError("text_too_large", "Extracted text exceeds the 500,000-byte limit. Use a smaller document.")
    return result


def export_docx(source, expected_text, replacements):
    """Return DOCX bytes using Python/Rust character offsets and exact tokens.

    Each replacement has start, end, text (original), and replacement (token).
    Validate the whole request before changing XML. Layout follows Word's reflow.
    """
    if not isinstance(source, bytes) or not source or len(source) > MAX_FILE_BYTES:
        raise DocumentError("invalid_file", "Choose a DOCX within the import size limit.")
    _check_archive(BytesIO(source))
    document = Document(BytesIO(source))
    mapped = extract_docx(document)
    if not isinstance(expected_text, str) or mapped.text != expected_text:
        raise DocumentError("stale_document", "The Word document no longer matches the processed text. Import and process it again.")
    if not isinstance(replacements, list) or not replacements:
        raise DocumentError("invalid_replacements", "Process at least one detection before DOCX export.")
    ordered = []
    for item in replacements:
        if not isinstance(item, dict):
            raise DocumentError("invalid_replacements", "Invalid DOCX replacement.")
        start, end = item.get("start"), item.get("end")
        token = item.get("replacement")
        if (type(start) is not int or type(end) is not int
                or not 0 <= start < end <= len(mapped.text)
                or mapped.text[start:end] != item.get("text")
                or not isinstance(token, str) or not token
                or len(token) > 1000 or any(ord(char) < 32 for char in token)):
            raise DocumentError("invalid_replacements", "Invalid or outdated DOCX replacement.")
        ordered.append(item)
    ordered.sort(key=lambda item: item["start"])
    if any(first["end"] > second["start"] for first, second in zip(ordered, ordered[1:])):
        raise DocumentError("overlap", "DOCX replacements overlap.")

    # One XML text node can have multiple appearances through merged cells.
    edits = {}
    node_starts = [node.start for node in mapped.nodes]
    node_ends = [node.end for node in mapped.nodes]
    for item in ordered:
        cursor = item["start"]
        first_node = True
        first = bisect_right(node_ends, item["start"])
        last = bisect_left(node_starts, item["end"])
        for node in mapped.nodes[first:last]:
            left, right = max(node.start, item["start"]), min(node.end, item["end"])
            if left != cursor:
                break
            edit = (left - node.start, right - node.start,
                    item["replacement"] if first_node else "")
            edits.setdefault(node.element, []).append(edit)
            first_node = False
            cursor = right
        if cursor != item["end"]:
            raise DocumentError("unsupported_span", "A selection crosses a paragraph, cell boundary, tab, or line break. Review smaller selections before DOCX export.")

    unique_edits = {element: sorted(set(values)) for element, values in edits.items()}
    # Require identical requested edits at every appearance of shared XML.
    replacement_starts = [item["start"] for item in ordered]
    replacement_ends = [item["end"] for item in ordered]
    for node in mapped.nodes:
        if node.element not in unique_edits:
            continue
        actual = []
        first = bisect_right(replacement_ends, node.start)
        last = bisect_left(replacement_starts, node.end)
        for item in ordered[first:last]:
            left, right = max(node.start, item["start"]), min(node.end, item["end"])
            if left < right:
                actual.append((left - node.start, right - node.start,
                               item["replacement"] if left == item["start"] else ""))
        if sorted(set(actual)) != unique_edits[node.element]:
            raise DocumentError("shared_cell_conflict", "A merged Word cell has conflicting review decisions. Apply the same replacements to all its repeated occurrences.")

    changed_parts = {}
    affected_links = {}
    for node in mapped.nodes:
        element = node.element
        if element not in unique_edits:
            continue
        value = _normalize(element.text or "")
        values = unique_edits.pop(element)
        # Replacing visible link text must not retain its original destination
        # or tooltip. Keep its styled runs, but remove the hyperlink wrapper.
        for ancestor in element.iterancestors(qn("w:hyperlink")):
            affected_links[ancestor] = node.part
        if any(first[1] > second[0] for first, second in zip(values, values[1:])):
            raise DocumentError("shared_cell_conflict", "Conflicting replacements affect the same Word text.")
        for start, end, token in reversed(values):
            value = value[:start] + token + value[end:]
        element.text = value
        element.set(qn("xml:space"), "preserve")
        changed_parts[str(node.part.partname).lstrip("/")] = node.part

    removed_relationships = {}
    for link, part in affected_links.items():
        relationship_id = link.get(qn("r:id"))
        if relationship_id:
            removed_relationships.setdefault(part, set()).add(relationship_id)
        parent = link.getparent()
        index = parent.index(link)
        for child in list(link):
            parent.insert(index, child)
            index += 1
        parent.remove(link)

    # Copy all unmodified ZIP members verbatim at the content level, including
    # styles, images, and fonts. Edited links also lose unused relationships.
    output = BytesIO()
    with ZipFile(BytesIO(source)) as original, ZipFile(output, "w") as protected:
        changed_relationships = {}
        for part, candidates in removed_relationships.items():
            used = {
                value for element in part._element.iter()
                for attribute, value in element.attrib.items()
                if attribute.startswith(qn("r:id").rsplit("}", 1)[0] + "}")
            }
            unused = candidates - used
            if not unused:
                continue
            name = str(part.partname).lstrip("/")
            rel_name = posixpath.join(posixpath.dirname(name), "_rels", posixpath.basename(name) + ".rels")
            root = etree.fromstring(original.read(rel_name), parser=etree.XMLParser(resolve_entities=False, no_network=True))
            for relationship in list(root):
                if relationship.get("Id") in unused:
                    root.remove(relationship)
            changed_relationships[rel_name] = etree.tostring(root, xml_declaration=True, encoding="UTF-8", standalone=True)
        for entry in original.infolist():
            part = changed_parts.get(entry.filename)
            data = changed_relationships.get(entry.filename)
            if data is None:
                data = part.blob if part is not None else original.read(entry)
            protected.writestr(entry, data)
    return output.getvalue()
