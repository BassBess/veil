"""Local-only language adapters. Model installation is a separate setup action."""
from pathlib import Path
import spacy
import phonenumbers
from presidio_analyzer import AnalyzerEngine, RecognizerRegistry
from presidio_analyzer.nlp_engine import SpacyNlpEngine
from presidio_analyzer.predefined_recognizers import (
    CreditCardRecognizer, DateRecognizer, IbanRecognizer, IpRecognizer,
    PhoneRecognizer, UrlRecognizer,
)

LANGUAGES = {"en": "English", "fr": "French", "es": "Spanish", "ar": "Arabic"}
SPACY_MODELS = {"en": "en_core_web_sm", "fr": "fr_core_news_sm", "es": "es_core_news_sm"}
MODEL_DIR = Path(__file__).resolve().parents[1] / "models" / "stanza"
ARABIC_PROCESSORS = {"tokenize": "padt", "mwt": "padt", "ner": "aqmar_nocharlm"}
CONTEXT = {
    "en": {"email": ["email", "mail"], "phone": ["phone", "telephone", "mobile"], "date": ["date", "birth", "born"]},
    "fr": {"email": ["courriel", "email", "mail"], "phone": ["téléphone", "portable", "tél"], "date": ["date", "naissance", "né"]},
    "es": {"email": ["correo", "electrónico", "email"], "phone": ["teléfono", "móvil", "celular"], "date": ["fecha", "nacimiento"]},
    "ar": {"email": ["بريد", "إلكتروني", "ايميل"], "phone": ["هاتف", "جوال", "موبايل"], "date": ["تاريخ", "ميلاد", "ولد"]},
}


class ModelUnavailable(ValueError):
    pass


def load_language(language, email_recognizer):
    if language not in LANGUAGES:
        raise ValueError("Choose English, French, Spanish, or Arabic.")
    arabic = None
    if language == "ar":
        try:
            import stanza
        except ImportError:
            raise ModelUnavailable("Arabic detection dependencies are missing. Run the multilingual model setup described in docs/multilingual-detection.md.") from None
        if not all(path.is_file() for path in [MODEL_DIR / "resources.json", *(MODEL_DIR / "ar" / processor / f"{package}.pt" for processor, package in ARABIC_PROCESSORS.items()), MODEL_DIR / "ar/pretrain/fasttextwiki.pt"]):
            raise ModelUnavailable("The local Arabic model is missing. Run python -m veil_engine.setup_models --languages ar from engines/python.")
        try:
            arabic = stanza.Pipeline("ar", dir=str(MODEL_DIR), processors=ARABIC_PROCESSORS,
                                     package=None, download_method=None, use_gpu=False, verbose=False)
        except (FileNotFoundError, ValueError, RuntimeError):
            raise ModelUnavailable("The local Arabic model could not load. Run python -m veil_engine.setup_models --languages ar from engines/python.") from None
        engine = SpacyNlpEngine(models=[{"lang_code": "ar", "model_name": "local-ar-tokenizer"}])
        # Supply the tokenizer directly: SpacyNlpEngine.load() must never download.
        engine.nlp = {"ar": spacy.blank("ar")}
    else:
        model = SPACY_MODELS[language]
        if not spacy.util.is_package(model):
            raise ModelUnavailable(f"The local {LANGUAGES[language]} model ({model}) is missing. Run python -m veil_engine.setup_models --languages {language} from engines/python.")
        engine = SpacyNlpEngine(models=[{"lang_code": language, "model_name": model}])
        engine.nlp = {language: spacy.load(model)}
    context = CONTEXT[language]
    registry = RecognizerRegistry(supported_languages=[language])
    registry.add_nlp_recognizer(nlp_engine=engine)
    for recognizer in [
        email_recognizer(supported_language=language, context=context["email"]),
        PhoneRecognizer(supported_language=language, context=context["phone"],
                        supported_regions=tuple(sorted(phonenumbers.SUPPORTED_REGIONS))),
        DateRecognizer(supported_language=language, context=context["date"]),
        CreditCardRecognizer(supported_language=language), IbanRecognizer(supported_language=language),
        IpRecognizer(supported_language=language), UrlRecognizer(supported_language=language),
    ]:
        registry.add_recognizer(recognizer)
    return AnalyzerEngine(nlp_engine=engine, registry=registry, supported_languages=[language]), arabic


def arabic_entities(pipeline, text, enabled):
    mapping = {"PER": "PERSON", "PERSON": "PERSON", "LOC": "LOCATION", "GPE": "LOCATION", "ORG": "ORGANIZATION"}
    if pipeline is None or not enabled.intersection(mapping.values()):
        return []
    return [{"entity_type": mapping[entity.type], "text": text[entity.start_char:entity.end_char],
             "start": entity.start_char, "end": entity.end_char, "confidence": .75}
            for entity in pipeline(text).ents if mapping.get(entity.type) in enabled]
