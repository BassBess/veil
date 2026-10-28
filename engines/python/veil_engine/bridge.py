import json
import sys

from veil_engine.detector import VeilDetector


def main():
    try:
        request = json.load(sys.stdin)

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