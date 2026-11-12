import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from docx import Document
from openpyxl import Workbook
from pypdf import PdfWriter
from pypdf.generic import DictionaryObject, NameObject, DecodedStreamObject

from veil_engine.document_parser import DocumentError, PDF_WARNING, parse_document

ENGINE_DIR = Path(__file__).resolve().parents[1]


class TestDocumentParser(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def write(self, name, data):
        path = self.root / name
        path.write_bytes(data)
        return path

    def assert_error(self, path, code):
        with self.assertRaises(DocumentError) as caught:
            parse_document(path)
        self.assertEqual(caught.exception.code, code)

    def test_txt_utf8_bom_and_unicode(self):
        path = self.write('ملاحظات.TXT', '\ufeffJohn 😀 مرحبا'.encode('utf-8'))
        document = parse_document(path)
        self.assertEqual(document['text'], 'John 😀 مرحبا')
        self.assertEqual(document['file_type'], 'txt')
        self.assertEqual(document['file_name'], path.name)
        self.assertIsNone(document['pages'])

    def test_windows_line_endings_match_textarea_positions(self):
        path = self.write('windows.txt', b'John\r\njohn@example.com\rLast line')
        self.assertEqual(parse_document(path)['text'], 'John\njohn@example.com\nLast line')

    def test_encoding_error(self):
        self.assert_error(self.write('legacy.txt', b'\xff\xfeinvalid'), 'encoding')

    def test_binary_txt(self):
        self.assert_error(self.write('binary.txt', b'abc\x00def'), 'invalid_file')

    def test_docx_headers_footers_and_linked_sections(self):
        document = Document()
        document.sections[0].header.paragraphs[0].text = "Contact Jane Smith"
        document.sections[0].footer.paragraphs[0].text = "jane@example.com"
        document.sections[0].first_page_header.paragraphs[0].text = "Private cover"
        document.sections[0].even_page_footer.paragraphs[0].text = "Office 555-0100"
        document.add_paragraph("Body text")
        document.add_section()
        document.add_paragraph("Second section")
        path = self.root / "headers.docx"
        document.save(path)

        text = parse_document(path)["text"]
        for value in ("Contact Jane Smith", "jane@example.com", "Private cover",
                      "Office 555-0100", "Body text", "Second section"):
            self.assertEqual(text.count(value), 1)
        self.assertLess(text.index("Contact Jane Smith"), text.index("Body text"))
        self.assertLess(text.index("Second section"), text.index("jane@example.com"))

    def test_docx_paragraph_table_order(self):
        document = Document()
        document.add_paragraph('Before')
        table = document.add_table(rows=1, cols=2)
        table.cell(0, 0).text = 'John Smith'
        table.cell(0, 1).text = 'john@example.com'
        document.add_paragraph('After')
        path = self.root / 'report.docx'
        document.save(path)
        self.assertEqual(parse_document(path)['text'], 'Before\nJohn Smith | john@example.com\nAfter')

    def test_csv_keeps_first_row_leading_zero_and_na(self):
        path = self.write('people.csv', b'Name,Phone,Note\nJohn,001234,NA\nJane,005678,"hello, world"')
        text = parse_document(path)['text']
        self.assertEqual(text, 'Name | Phone | Note\nJohn | 001234 | NA\nJane | 005678 | hello, world')

    def test_xlsx_all_sheets(self):
        workbook = Workbook()
        workbook.active.title = 'People'
        workbook.active.append(['Name', 'Phone'])
        workbook.active.append(['John', '001234'])
        workbook.create_sheet('Contacts').append(['john@example.com'])
        path = self.root / 'people.xlsx'
        workbook.save(path)
        self.assertEqual(parse_document(path)['text'], 'Sheet: People\nName | Phone\nJohn | 001234\nSheet: Contacts\njohn@example.com')

    def pdf(self, name, text=None):
        writer = PdfWriter()
        page = writer.add_blank_page(width=300, height=300)
        if text:
            font = DictionaryObject({NameObject('/Type'): NameObject('/Font'), NameObject('/Subtype'): NameObject('/Type1'), NameObject('/BaseFont'): NameObject('/Helvetica')})
            page[NameObject('/Resources')] = DictionaryObject({NameObject('/Font'): DictionaryObject({NameObject('/F1'): writer._add_object(font)})})
            stream = DecodedStreamObject()
            stream.set_data(f'BT /F1 12 Tf 20 200 Td ({text}) Tj ET'.encode('ascii'))
            page[NameObject('/Contents')] = writer._add_object(stream)
        path = self.root / name
        writer.write(path)
        return path

    def test_pdf_with_text(self):
        document = parse_document(self.pdf('report.pdf', 'Contact john@example.com'))
        self.assertIn('john@example.com', document['text'])
        self.assertEqual(document['pages'], 1)
        self.assertEqual(document['warnings'], [])

    def test_pdf_no_extractable_text_warns(self):
        document = parse_document(self.pdf('scan.pdf'))
        self.assertEqual(document['text'], '')
        self.assertIn(PDF_WARNING, document['warnings'])

    def test_encrypted_pdf(self):
        writer = PdfWriter()
        writer.add_blank_page(width=300, height=300)
        writer.encrypt('secret')
        path = self.root / 'protected.pdf'
        writer.write(path)
        self.assert_error(path, 'encrypted')

    def test_unsupported(self):
        self.assert_error(self.write('file.exe', b'hello'), 'unsupported_type')

    def test_missing(self):
        self.assert_error(self.root / 'gone.txt', 'missing_file')

    def test_unreadable(self):
        path = self.write('locked.txt', b'hello')
        with patch.object(Path, 'read_bytes', side_effect=PermissionError):
            self.assert_error(path, 'unreadable_file')

    def test_malformed_formats(self):
        for extension in ['docx', 'pdf', 'xlsx', 'csv']:
            with self.subTest(extension=extension):
                self.assert_error(self.write(f'broken.{extension}', b'"unclosed' if extension == 'csv' else b'not a document'), 'invalid_file')

    def test_empty_and_oversized(self):
        self.assert_error(self.write('empty.txt', b''), 'empty_file')
        self.assert_error(self.write('long.txt', b'a' * 100_001), 'text_too_large')
        with patch('veil_engine.document_parser.MAX_FILE_BYTES', 2):
            self.assert_error(self.write('big.txt', b'abc'), 'file_too_large')

    def test_archive_expansion_limit(self):
        document = Document()
        document.add_paragraph('John')
        path = self.root / 'expanded.docx'
        document.save(path)
        with patch('veil_engine.document_parser.MAX_ARCHIVE_BYTES', 1):
            self.assert_error(path, 'archive_too_large')

    def test_whitespace_only(self):
        self.assert_error(self.write('blank.txt', b'   \n'), 'empty_document')

    def test_pdf_sparse_text_warns(self):
        result = parse_document(self.pdf('sparse.pdf', 'Hi'))
        self.assertIn('Hi', result['text'])
        self.assertIn(PDF_WARNING, result['warnings'])

    def bridge(self, request):
        # Fail the integration test if any stage attempts an outbound connection.
        offline_bridge = """
import runpy
import socket
attempts = []
def forbidden_connect(*args, **kwargs):
    attempts.append(True)
    raise RuntimeError('Network access is forbidden in this test')
socket.socket.connect = forbidden_connect
runpy.run_module('veil_engine.bridge', run_name='__main__')
if attempts:
    raise RuntimeError('The bridge attempted network access')
"""
        completed = subprocess.run([sys.executable, '-c', offline_bridge], input=json.dumps(request), text=True,
                                   capture_output=True, cwd=ENGINE_DIR, env={**os.environ, 'PYTHONUTF8': '1'}, timeout=60)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        return json.loads(completed.stdout)

    def test_bridge_structured_error(self):
        response = self.bridge({'action': 'parse_document', 'file_path': str(self.root / 'gone.txt')})
        self.assertEqual(response['error']['code'], 'missing_file')

    def test_imported_text_existing_detection_bridge(self):
        path = self.write('contacts.txt', b'Contact john@example.com for details.')
        imported = self.bridge({'action': 'parse_document', 'file_path': str(path)})['document']
        result = self.bridge({'text': imported['text'], 'entities': ['EMAIL_ADDRESS']})
        self.assertTrue(any(item['text'] == 'john@example.com' and item['entity_type'] == 'EMAIL_ADDRESS' for item in result['detections']))
        for item in result['detections']:
            self.assertEqual(imported['text'][item['start']:item['end']], item['text'])


if __name__ == '__main__':
    unittest.main()
