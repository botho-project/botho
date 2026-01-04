# Makefile for Botho Whitepaper
# Requires: pdflatex, bibtex (or biber)

MAIN = botho-whitepaper
SECTIONS = $(wildcard sections/*.tex)
BIB = refs.bib

.PHONY: all clean distclean view

all: $(MAIN).pdf

$(MAIN).pdf: $(MAIN).tex preamble.tex $(SECTIONS) $(BIB)
	pdflatex $(MAIN)
	bibtex $(MAIN)
	pdflatex $(MAIN)
	pdflatex $(MAIN)

# Quick build (no bibliography update)
quick: $(MAIN).tex preamble.tex $(SECTIONS)
	pdflatex $(MAIN)

# Clean auxiliary files
clean:
	rm -f *.aux *.log *.bbl *.blg *.out *.toc *.lof *.lot
	rm -f sections/*.aux

# Clean everything including PDF
distclean: clean
	rm -f $(MAIN).pdf

# View the PDF (macOS)
view: $(MAIN).pdf
	open $(MAIN).pdf

# View the PDF (Linux)
view-linux: $(MAIN).pdf
	xdg-open $(MAIN).pdf

# Continuous build (requires latexmk)
watch:
	latexmk -pdf -pvc $(MAIN)

# Word count (approximate)
wordcount:
	@texcount -inc $(MAIN).tex | grep "Words in text"

# Check for common LaTeX issues
check:
	@echo "Checking for common issues..."
	@grep -n "TODO" $(MAIN).tex $(SECTIONS) || echo "No TODOs found"
	@grep -n "FIXME" $(MAIN).tex $(SECTIONS) || echo "No FIXMEs found"
	@grep -n "\\\\cite{}" $(MAIN).tex $(SECTIONS) || echo "No empty citations"
