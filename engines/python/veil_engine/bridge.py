import json
import sys


def main():
    try:
        request = json.load(sys.stdin)

        if request.get("action") == "parse_document":
            from veil_engine.document_parser import DocumentError, parse_document
            try:
                result = parse_document(request["file_path"])
                json.dump({"document": result}, sys.stdout)
            except DocumentError as error:
                json.dump({"error": {"code": error.code, "message": str(error)}}, sys.stdout)
            return

        from veil_engine.detector import VeilDetector
        text = request["text"]
        entities = request.get("entities")

        detector = VeilDetector()

        results = detector.suggest(
            text,
            entities,
        )

        json.dump(
            {
                "detections": results
            },
            sys.stdout,
        )

    except Exception:
        print(
            "Detection request failed",
            file=sys.stderr,
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
