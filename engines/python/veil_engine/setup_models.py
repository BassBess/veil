"""Explicit network-enabled setup; detection itself never downloads models."""
import argparse
import subprocess
import sys
from veil_engine.languages import LANGUAGES, SPACY_MODELS, MODEL_DIR, ARABIC_PROCESSORS


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--languages", nargs="+", choices=list(LANGUAGES), default=list(LANGUAGES))
    args = parser.parse_args()
    for language in args.languages:
        if language == "ar":
            import stanza
            stanza.download("ar", model_dir=str(MODEL_DIR), processors=ARABIC_PROCESSORS, package=None)
        else:
            model = SPACY_MODELS[language]
            url = f"https://github.com/explosion/spacy-models/releases/download/{model}-3.8.0/{model}-3.8.0-py3-none-any.whl"
            subprocess.run([sys.executable, "-m", "pip", "install", url], check=True)


if __name__ == "__main__":
    main()
