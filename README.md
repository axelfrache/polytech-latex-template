# Polytech LaTeX Template

A simple LaTeX template for Polytech Montpellier project reports, with a title page, table of contents, page headers, and automated PDF builds.

## Usage

Edit the document metadata at the top of [`rapport.tex`](rapport.tex), then replace the example content in [`sections/section.tex`](sections/section.tex).

Add new sections with:

```latex
\include{sections/section-name}
```

## Build

Install a LaTeX distribution with `latexmk`, then run:

```bash
latexmk -pdf rapport.tex
```

The generated file is `rapport.pdf`. GitHub Actions also builds and uploads it on pushes and pull requests to `main`.
