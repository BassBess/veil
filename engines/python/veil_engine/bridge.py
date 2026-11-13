import json
import sys


def main():
    try:
        request = json.load(sys.stdin)

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
