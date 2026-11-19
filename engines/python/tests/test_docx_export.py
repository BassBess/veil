import base64
import json
from pathlib import Path
import os
import subprocess
import sys
import unittest
from io import BytesIO
from zipfile import ZipFile

from docx import Document
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.opc.constants import RELATIONSHIP_TYPE as RT
from docx.shared import Pt
from veil_engine.document_parser import DocumentError
from veil_engine.docx_export import export_docx, extract_docx


def encoded(document):
    stream = BytesIO()
    document.save(stream)
    return stream.getvalue()


def replacement(text, value, token, offset=0):
    start = text.index(value, offset)
    return dict(start=start, end=start + len(value), text=value, replacement=token)


class TestDocxExport(unittest.TestCase):
    def test_replaced_hyperlink_loses_destination_and_tooltip_but_keeps_style(self):
        for in_header in [False, True]:
            with self.subTest(in_header=in_header):
                document = Document()
                paragraph = document.sections[0].header.paragraphs[0] if in_header else document.add_paragraph()
                link = OxmlElement('w:hyperlink')
                rid = paragraph.part.relate_to('mailto:john@example.com', RT.HYPERLINK, is_external=True)
                link.set(qn('r:id'), rid)
                link.set(qn('w:tooltip'), 'Email john@example.com')
                run = paragraph.add_run('john@example.com')
                run.bold = True
                link.append(run._r)
                paragraph._p.append(link)
                text = extract_docx(document).text
                result = export_docx(encoded(document), text, [replacement(text, text, 'EMAIL_001')])
                output = Document(BytesIO(result))
                paragraph = output.sections[0].header.paragraphs[0] if in_header else output.paragraphs[0]
                self.assertEqual(paragraph.text, 'EMAIL_001')
                self.assertTrue(paragraph.runs[0].bold)
                self.assertFalse(paragraph.hyperlinks)
                with ZipFile(BytesIO(result)) as archive:
                    for name in archive.namelist():
                        if name.endswith(('.xml', '.rels')):
                            self.assertNotIn(b'john@example.com', archive.read(name), name)

    def test_unselected_link_sharing_destination_remains_unchanged(self):
        document = Document()
        paragraph = document.add_paragraph()
        for index in range(2):
            if index:
                paragraph.add_run(' and ')
            link = OxmlElement('w:hyperlink')
            rid = paragraph.part.relate_to('mailto:john@example.com', RT.HYPERLINK, is_external=True)
            link.set(qn('r:id'), rid)
            run = paragraph.add_run('john@example.com')
            link.append(run._r)
            paragraph._p.append(link)
        text = extract_docx(document).text
        result = export_docx(encoded(document), text, [replacement(text, 'john@example.com', 'EMAIL_001')])
        output = Document(BytesIO(result))
        self.assertEqual(output.paragraphs[0].text, 'EMAIL_001 and john@example.com')
        self.assertEqual(len(output.paragraphs[0].hyperlinks), 1)
        self.assertEqual(output.paragraphs[0].hyperlinks[0].address, 'mailto:john@example.com')

    def test_split_runs_preserve_style_and_surrounding_content(self):
        document = Document()
        paragraph = document.add_paragraph()
        paragraph.add_run('Hello ')
        name = paragraph.add_run('John ')
        name.bold = True
        name.font.name = 'Arial'
        name.font.size = Pt(14)
        paragraph.add_run('Smith').italic = True
        paragraph.add_run(', welcome.')
        source = encoded(document)
        text = extract_docx(document).text
        result = export_docx(source, text, [replacement(text, 'John Smith', 'PERSON_001')])
        output = Document(BytesIO(result))
        self.assertEqual(output.paragraphs[0].text, 'Hello PERSON_001, welcome.')
        run = output.paragraphs[0].runs[1]
        self.assertTrue(run.bold)
        self.assertEqual(run.font.name, 'Arial')
        self.assertEqual(run.font.size, Pt(14))
        with ZipFile(BytesIO(source)) as before, ZipFile(BytesIO(result)) as after:
            self.assertEqual(before.namelist(), after.namelist())
            for name in before.namelist():
                if name != 'word/document.xml':
                    self.assertEqual(before.read(name), after.read(name), name)

    def test_only_selected_repeated_occurrence_changes_with_unicode(self):
        document = Document()
        document.add_paragraph('😀 مرحبا John and John')
        text = extract_docx(document).text
        item = replacement(text, 'John', 'PERSON_001', text.index('John') + 4)
        result = Document(BytesIO(export_docx(encoded(document), text, [item])))
        self.assertEqual(result.paragraphs[0].text, '😀 مرحبا John and PERSON_001')

    def test_headers_tables_and_footers_share_import_order(self):
        document = Document()
        document.sections[0].header.paragraphs[0].text = 'Jane'
        document.add_paragraph('Body')
        table = document.add_table(rows=1, cols=3)
        table.cell(0, 0).text = 'John'
        table.cell(0, 2).text = 'Mira'
        document.sections[0].footer.paragraphs[0].text = 'Contact Jane'
        document.add_section()
        text = extract_docx(document).text
        self.assertEqual(text, 'Jane\nBody\nJohn |  | Mira\nContact Jane')
        items = [replacement(text, 'Jane', 'PERSON_001'), replacement(text, 'Mira', 'PERSON_002')]
        output = Document(BytesIO(export_docx(encoded(document), text, items)))
        self.assertEqual(output.sections[0].header.paragraphs[0].text, 'PERSON_001')
        self.assertEqual(output.tables[0].cell(0, 2).text, 'PERSON_002')
        self.assertEqual(output.sections[0].footer.paragraphs[0].text, 'Contact Jane')

    def test_multiple_adjacent_replacements_in_same_node(self):
        document = Document()
        document.add_paragraph('JohnSmith')
        text = extract_docx(document).text
        items = [replacement(text, 'John', 'FIRST'), replacement(text, 'Smith', 'LAST')]
        output = Document(BytesIO(export_docx(encoded(document), text, items)))
        self.assertEqual(output.paragraphs[0].text, 'FIRSTLAST')

    def test_stale_invalid_overlap_and_cross_paragraph_requests_fail(self):
        document = Document()
        document.add_paragraph('John')
        document.add_paragraph('Smith')
        source = encoded(document)
        text = extract_docx(document).text
        valid = replacement(text, 'John', 'PERSON_001')
        requests = [
            ('Changed', [valid]),
            (text, [{**valid, 'text': 'Wrong'}]),
            (text, [{**valid, 'start': -1}]),
            (text, [valid, valid]),
            (text, [replacement(text, text, 'PERSON_001')]),
        ]
        for expected, items in requests:
            with self.subTest(expected=expected, items=items), self.assertRaises(DocumentError):
                export_docx(source, expected, items)
        self.assertEqual(Document(BytesIO(source)).paragraphs[0].text, 'John')

    def test_shared_merged_cell_requires_consistent_decisions(self):
        document = Document()
        table = document.add_table(rows=1, cols=2)
        table.cell(0, 0).merge(table.cell(0, 1)).text = 'John'
        source = encoded(document)
        text = extract_docx(document).text
        self.assertEqual(text, 'John | John')
        first = replacement(text, 'John', 'PERSON_001')
        second = replacement(text, 'John', 'PERSON_001', 4)
        with self.assertRaises(DocumentError) as error:
            export_docx(source, text, [first])
        self.assertEqual(error.exception.code, 'shared_cell_conflict')
        output = Document(BytesIO(export_docx(source, text, [first, second])))
        self.assertEqual(output.tables[0].cell(0, 0).text, 'PERSON_001')

    def test_line_break_is_preserved_and_crossing_it_is_rejected(self):
        document = Document()
        document.add_paragraph('John\tSmith\nNext')
        source = encoded(document)
        text = extract_docx(document).text
        output = Document(BytesIO(export_docx(source, text, [replacement(text, 'Smith', 'PERSON_001')])))
        self.assertEqual(output.paragraphs[0].text, 'John\tPERSON_001\nNext')
        with self.assertRaises(DocumentError):
            export_docx(source, text, [replacement(text, 'John\tSmith', 'PERSON_001')])


class TestDocxExportBridge(unittest.TestCase):
    def bridge(self, request):
        script = """
import runpy
import socket
import sys
attempts = []
def forbidden(*args, **kwargs):
    attempts.append(True)
    raise RuntimeError('Network access is forbidden')
socket.socket.connect = forbidden
runpy.run_module('veil_engine.bridge', run_name='__main__')
if attempts or 'veil_engine.detector' in sys.modules:
    raise RuntimeError('DOCX export must be offline and independent of detection')
"""
        result = subprocess.run(
            [sys.executable, '-c', script], input=json.dumps(request),
            text=True, capture_output=True,
            cwd=Path(__file__).resolve().parents[1],
            env={**os.environ, 'PYTHONUTF8': '1'}, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def request(self):
        document = Document()
        document.add_paragraph('John and John')
        text = extract_docx(document).text
        return {
            'action': 'export_docx',
            'source_docx': base64.b64encode(encoded(document)).decode('ascii'),
            'text': text,
            'replacements': [replacement(text, 'John', 'PERSON_001', 5)],
        }

    def test_exports_real_docx_offline_without_loading_detector(self):
        response = self.bridge(self.request())
        output = Document(BytesIO(base64.b64decode(response['docx'], validate=True)))
        self.assertEqual(output.paragraphs[0].text, 'John and PERSON_001')

    def test_stale_source_returns_structured_error(self):
        request = self.request()
        request['text'] = 'Edited document'
        response = self.bridge(request)
        self.assertEqual(response['error']['code'], 'stale_document')
        self.assertNotIn('docx', response)

    def test_missing_and_invalid_source_return_structured_errors(self):
        for source in [None, 'invalid base64!']:
            with self.subTest(source=source):
                response = self.bridge({**self.request(), 'source_docx': source})
                self.assertEqual(response['error']['code'], 'invalid_file')
                self.assertNotIn('docx', response)


if __name__ == '__main__':
    unittest.main()
