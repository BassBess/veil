"""Render only the reviewed, processed text into a new local PDF."""
import html
import io
import pymupdf


def export_pdf(text):
    if not isinstance(text, str) or not text.strip():
        raise ValueError("There is no processed text to export.")
    if len(text.encode("utf-8")) > 5_000_000:
        raise ValueError("Processed text exceeds the export size limit.")
    # Escape all markup so document contents cannot load resources or become HTML.
    paragraphs = "".join(
        "<p>" + (html.escape(line) or "&#160;") + "</p>"
        for line in text.split("\n")
    )
    story = pymupdf.Story(
        html=paragraphs,
        user_css="body {font-family:sans-serif;font-size:11pt;} p {white-space:pre-wrap;overflow-wrap:anywhere;margin:0 0 6pt;}",
    )
    stream = io.BytesIO()
    page = pymupdf.Rect(0, 0, 595, 842)
    area = pymupdf.Rect(42, 42, 553, 800)
    with pymupdf.DocumentWriter(stream) as writer:
        for _ in range(1000):
            device = writer.begin_page(page)
            more, _ = story.place(area)
            story.draw(device)
            writer.end_page()
            if not more:
                break
        else:
            raise ValueError("The PDF exceeds the 1,000-page export limit.")
    return stream.getvalue()
