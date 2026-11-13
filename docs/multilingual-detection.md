# Multilingual detection investigation

Status: initial implementation, 2026-11-13. The research and follow-up evaluation
plan below explain the design and remaining work.

## Using the implementation

From `engines/python` on macOS:

```sh
.venv/bin/python -m pip install -r requirements.txt
.venv/bin/python -m veil_engine.setup_models --languages ar
```

On Windows, use `.venv/Scripts/python.exe` in both commands. Requirements install
the English, French, and Spanish spaCy 3.8.0 model packages and Stanza 1.11.0.
The separate setup command downloads Arabic PADT tokenization/MWT and AQMAR
`aqmar_nocharlm` NER with its FastText word vectors into `engines/python/models/`.
That folder is ignored by Git. These model files must be installed on every
machine; pulling the source does not download them. Setup requires internet.
Detection explicitly disables model downloads and loads only the chosen language.

Choose **Document language** before detecting. All four language modes feed the
same review, anonymization, and pseudonymization pipeline. The output token names
remain English (`PERSON_001`, etc.), regardless of document language.

The document rules editor has been removed from the interface for now. Detection
requests send only the selected language in their options; there are no
workspace inclusion, exclusion, or required-context controls. The internal rule
helpers remain available for future work and saved Vault phrases are unchanged.
Changing the document language or category selection invalidates the review.

Scores are displayed for review but never used as a minimum-score filter.
They include fixed recognizer strengths and are not calibrated probabilities.
Arabic NER receives a fixed 0.75 review score because the Stanza entity API does
not expose a comparable entity probability.
Suppressed suggestions are listed with their reason before processing. Models
also suppress localized field labels immediately before a colon (for example
`Courriel:` misclassified as a place); explicit custom inclusions can override
that heuristic. Manual review additions can override missed/excluded spans.

Arabic/Persian digits are mapped one-for-one for structured recognizers only;
all returned spans reference the unchanged source text. Arabic NER runs on the
original string. Input/output text areas use automatic direction detection.

### Current limitations

- Explicit language selection; automatic detection and mixed-language routing
  remain follow-up work. A Latin email inside Arabic is supported, but this does
  not imply full code-switching NER.
- Arabic Stanza AQMAR without character LMs is the initial local CPU baseline,
  not the winner of a comparative model benchmark. CAMeLBERT comparison and
  native-speaker-reviewed precision/recall evaluation remain outstanding.
- Automatic indirect-description linguistic rules still cover English only.
  Custom phrases work in all four languages.
- Localized ID/address/postal/date patterns are conservative initial rules, not
  exhaustive validation for every country. Phone recognition automatically checks
  every region supported by the installed phone-number library, including
  international numbers with country codes. There is no region selector.
  Number validation still applies; this does not guarantee recognition of every
  possible formatting variation or fictional number.
- A fresh Python process loads the selected model for each request. Long Arabic
  documents may hit the existing three-minute timeout. Model reuse and broader
  performance evaluation remain follow-up work.
- Standalone model packaging, native RTL selection checks, and visual PDF
  verification on both operating systems still require release validation.

### Checks

From `engines/python`, after installing all models:

```sh
.venv/bin/python -m unittest discover -s tests -v
```

The multilingual integration tests deliberately fail rather than skip when
models are absent. From the repository root, the Rust bridge integration can be
run with:

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml multilingual_detection_rules_and_both_processing_modes -- --include-ignored
```

These smoke examples verify wiring, offline inference, representative spans,
rules, and processing. They do not establish production detection accuracy.

Validation on the development Mac: 57 Python tests passed, including all four
installed language models with socket connections blocked; 50 Rust tests passed
with Python integrations enabled and the three OS Keychain tests excluded;
20 frontend review tests passed; frontend build, lint, and Rust compilation passed.
The Keychain tests require native credential-store access and are unrelated to
language routing. Mixed Arabic/Latin PDF generation also ran locally with network
connections blocked; visual reading order still needs desktop review. Windows
runtime validation and native-speaker-reviewed accuracy evaluation remain open.

## Requirement

Support English, French, Spanish, and Arabic, with configurable detection rules
that reduce irrelevant replacements. The detection and review pipeline should
serve both anonymization and pseudonymization. Anonymization must remain usable
without opening or storing anything in the Vault.

## Starting implementation (before this change)

- `engines/python/veil_engine/detector.py` loads only `en_core_web_sm` and passes
  `en` to every Presidio and spaCy operation.
- The frontend, Rust request, and Python bridge carry categories but no language.
- Structured recognizers and contextual words need explicit per-language
  registration; adding a model alone is insufficient.
- `context_rules.py` contains some multilingual address labels, but ID labels and
  linguistic indirect-identifier rules are predominantly English. Its noun-chunk
  and `NORP` assumptions cannot be reused across all languages and models.
- Custom literal phrases add indirect-identifier suggestions. There are no
  exclusion rules, user-configurable context requirements, or score thresholds.
- Custom rules are currently persisted in the encrypted Vault and loaded only
  while it is unlocked.
- Each detection request starts a fresh Python process and loads its model.
  Adding several eagerly loaded models would increase startup time and memory.
- Existing Unicode spans, chunking, review, token replacement, and export paths
  can be retained, but need multilingual regression tests.

## Recommended first implementation

1. Add an explicit document-language selector: English, French, Spanish, Arabic.
   Pass an allowlisted language code through React -> Rust -> Python. Keep English
   as the backward-compatible default for existing callers and tests. Language
   selection describes document language, not interface language or country.
2. Retain spaCy for English. Evaluate `fr_core_news_sm` and `es_core_news_sm` as
   initial lightweight baselines. Map PER/PERSON, LOC/GPE, and ORG into Veil's
   PERSON, LOCATION, and ORGANIZATION categories. Do not map MISC indiscriminately.
3. Benchmark Arabic Stanza NER and CAMeLBERT MSA NER on the same annotated sample
   set before choosing. Arabic tokenization support alone does not provide NER.
   The CAMeLBERT candidate is trained for Modern Standard Arabic; do not assume
   equivalent quality on Lebanese dialect, Arabizi, or mixed Arabic/Latin text.
4. Register the selected language's structured recognizers and localized context
   terms for email, phone, date, address, postal code, and labeled IDs. Define
   country-specific ID patterns separately; phone recognition checks all supported regions.
5. Load only the requested model. Preinstall version-pinned model assets and
   prohibit automatic downloads during document processing. Report missing models
   explicitly; never silently process French or Arabic using the English model.
6. Preserve the existing detection response shape and processing pipeline so both
   modes receive the same reviewed spans. Model-specific adapters should return
   original-text offsets, category, score when meaningful, and detection source.
   Do not present fixed recognizer scores as calibrated probabilities.

## Configurable rules

Provide simple controls first, with advanced options only when needed:

- Existing category switches.
- Custom literal phrases with an explicit category and optional language scope.
- Ignore phrases scoped to a category and document or reusable profile.
- Context requirements, such as accepting a number as an ID only near a passport,
  patient-ID, or employee-ID label in the selected language.
- Optional locale selection for country-specific structured identifiers.

Keep suggestions separate from replacement: only the reviewed selection or the
user's explicit process-all action should replace text. Exclusions should be
inspectable, with a way to view suppressed results. An ambiguous phrase such as
"Orange" should not acquire a global ignore rule after one rejection.

Suggested precedence: enabled categories and language scope -> candidate generation
-> validated context rules -> scoped exclusions -> overlap resolution ->
review. A manual addition should remain possible for excluded or missed spans.
Changing language or rules invalidates the current detection preview; require
redetection rather than silently altering accepted decisions.

Document-only rules should live in memory and work without a Vault. Reusable
phrase profiles may contain personal information: persisting them should be
explicit and encrypted. Existing Vault-backed rules can remain a reusable option,
but must not become a requirement for configuring an anonymization session.

## Arabic and mixed-language requirements

- Preserve the source string; normalizing Arabic letters, removing diacritics, or
  changing digits can invalidate character offsets. If a model requires a changed
  representation, maintain an explicit mapping back to original source spans.
- Test diacritics, Arabic and Persian digit forms, attached conjunctions and
  prepositions, Arabic punctuation, emoji, and Latin email addresses in RTL text.
- Use `dir="auto"` where suitable in text input/output and isolate LTR tokens in
  RTL presentation. Test native text selection and PDF reading order separately.
- Follow explicit-language support with automatic language suggestions and a
  manual override. Mixed documents need segment routing with original offsets;
  one dominant-language prediction is not sufficient for code-switched text.
- Do not translate documents before detecting: translation can change names,
  remove identifiers, and lose correspondence with source spans.

## Evaluation and delivery

Build an annotated test set in all four languages with native-speaker review.
Include names, organizations, locations, structured identifiers, ordinary words
that resemble names, technical numbers, non-personal dates, mixed scripts, and
repeated entities. Keep development and held-out evaluation samples separate.

Measure precision, recall, and span-boundary accuracy by language and category,
plus cold-start latency, processing latency, and peak memory on the target Mac
and Windows machines. Use the same dataset to compare Arabic candidates; public
benchmark scores from different datasets are not a reliable model ranking.

Test integration through both modes: repeated values share tokens, distinct values
receive different tokens, anonymization resets its mapping for each processing
request and does not persist it, and pseudonymization retains Vault mappings.
Test TXT/PDF import and export, grouped review, exact selection, and long-document
chunk boundaries with Arabic and accented text. Test offline behavior with
network access disabled and model-missing failures.

Delivery order:

1. Annotated evaluation fixtures and language-aware request plumbing.
2. English/French/Spanish baseline and localized structured rules.
3. Arabic benchmark and selected adapter, Unicode/RTL verification.
4. Session-level configurable inclusions, exclusions, and context rules.
5. Optional auto-detection and mixed-language routing, after explicit modes pass.

## Primary references

- Presidio language configuration (NLP models and recognizers both need adaptation):
  https://github.com/microsoft/presidio/blob/main/docs/analyzer/languages.md
- spaCy French trained pipelines: https://spacy.io/models/fr
- spaCy Spanish trained pipelines: https://spacy.io/models/es
- spaCy language support (Arabic has no official trained pipeline):
  https://spacy.io/usage/models#languages
- Stanza NER models, including Arabic AQMAR:
  https://stanfordnlp.github.io/stanza/ner_models.html
- Stanza NER API and original character spans:
  https://stanfordnlp.github.io/stanza/ner.html
- CAMeLBERT MSA NER model card, dataset, and usage:
  https://huggingface.co/CAMeL-Lab/bert-base-arabic-camelbert-msa-ner
- CAMeL Tools NER API: https://camel-tools.readthedocs.io/en/latest/api/ner.html
