These minimal OOXML fixtures are authored independently of the Contrix exporters and contain no customXml native model. external-formulas.xlsx contains A1=1 and B1=A1+1 with a cached result. external-slide-order.pptx lists slide2 before slide1 and includes an unreferenced slide3, plus an unsupported picture. They exercise the actual compatibility import paths.

`writer-typography.docx` is a synthetic, independently authored document with inherited SimSun typography, a centered 16pt title, 1.5 line spacing, two-character first-line indentation, Chinese numbering starting at two, and an explicit numbering cancellation. It contains no user document content.
`writer-typography-grid.docx` adds a 312-twip Word document line grid to the same synthetic content.

`external-rich-reordered.pptx` combines the independently authored rich slide1 with the plain slide2 from the order fixture. Its presentation relationships put slide2 first; the second slide retains the image, chart, shape, coordinates and explicit unsupported features from `external-rich.pptx`. Rust and browser regressions use the same bytes to check page order, rich elements, degradation warnings and exact original-file recovery. `pptx_reordered()` in the generator rebuilds only this combined fixture using fixed ZIP headers.

`reused-large-background.pptx` uses the same PNG background on 20 slides, with centered white titles and a narrow, right-aligned text box. The PNG exceeds 2 MiB before ZIP compression, reproducing the previous missing-background import. `generate_pptx_media_fixture.py` rebuilds it with Python's standard library and fixed ZIP headers; it contains only synthetic content.

`password-reader.pdf` is a synthetic single-page PDF encrypted with AES-256, reader password `reader123` and owner password `owner123`. It checks password-protected reading without enabling editing or server-side conversion. It contains no user document content.
