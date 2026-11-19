import json
import sys


def main():
    try:
        request = json.load(sys.stdin)

        if request.get("action") == "export_docx":
            import base64
            import binascii
            from veil_engine.document_parser import DocumentError, MAX_FILE_BYTES
            from veil_engine.docx_export import export_docx
            try:
                source = request.get("source_docx")
                if not isinstance(source, str) or len(source) > 4 * ((MAX_FILE_BYTES + 2) // 3):
                    raise DocumentError("invalid_file", "Choose a DOCX within the import size limit.")
                try:
                    original = base64.b64decode(source, validate=True)
                except (ValueError, binascii.Error):
                    raise DocumentError("invalid_file", "The original DOCX data is invalid. Import it again.") from None
                data = export_docx(original, request.get("text"), request.get("replacements"))
                json.dump({"docx": base64.b64encode(data).decode("ascii")}, sys.stdout)
            except DocumentError as error:
                json.dump({"error": {"code": error.code, "message": str(error)}}, sys.stdout)
            return

        if request.get("action") == "export_pdf":
            import base64
            from veil_engine.pdf_export import export_pdf
            data = export_pdf(request["text"])
            json.dump({"pdf": base64.b64encode(data).decode("ascii")}, sys.stdout)
            return

        if request.get("action") == "parse_document":
            from veil_engine.document_parser import DocumentError, parse_document
            try:
                result = parse_document(request["file_path"], request.get("max_file_bytes"))
                json.dump({"document": result}, sys.stdout)
            except DocumentError as error:
                json.dump({"error": {"code": error.code, "message": str(error)}}, sys.stdout)
            return

        from veil_engine.detector import VeilDetector
        text = request["text"]
        entities = request.get("entities")

        detector = VeilDetector(request.get("language", "en"))

        results = detector.suggest(
            text,
            entities,
            request.get("rules"),
            request.get("options"),
        )

        json.dump(
            {
                "detections": results,
                "suppressed": detector.suppressed
            },
            sys.stdout,
        )

    except ValueError as error:
        # These errors contain validated setup/rule messages, never document text.
        json.dump({"error": {"message": str(error)}}, sys.stdout)
    except Exception:
        print(
            "Detection request failed",
            file=sys.stderr,
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
