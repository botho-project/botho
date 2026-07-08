%% anvil-paper.cls — Default LaTeX class for anvil:pub
%%
%% Minimal generic single-column research-paper class. Compiles cleanly with
%% pdflatex + bibtex from a fresh checkout, no venue-specific assumptions.
%% Consumers override by dropping a venue-specific .cls/.sty into
%% .anvil/skills/pub/templates/ in their own repo and setting the brief's
%% `documentclass:` frontmatter accordingly.
%%
%% Options:
%%   anonymous  Suppress author/affiliation rendering for double-blind submission.
%%
%% Provides:
%%   \title{}, \author{}, \date{}, \abstract{}
%%   Standard \section, \subsection, \subsubsection
%%   natbib (\citep, \citet) loaded with numbers + sort&compress
%%   graphicx, amsmath, amssymb, xcolor, hyperref loaded
%%   plainnat bibliography style baked in

\NeedsTeXFormat{LaTeX2e}
\ProvidesClass{anvil-paper}[2026/05/28 Anvil generic paper class]

%% Option handling ---------------------------------------------------------
\newif\ifanvil@anonymous
\anvil@anonymousfalse
\DeclareOption{anonymous}{\anvil@anonymoustrue}
\DeclareOption*{\PassOptionsToClass{\CurrentOption}{article}}
\ProcessOptions\relax

%% Base class --------------------------------------------------------------
\LoadClass[11pt,letterpaper]{article}

%% Geometry: reasonable margins for readability + comfortable line length
\RequirePackage[margin=1in]{geometry}

%% Common packages ---------------------------------------------------------
\RequirePackage{graphicx}
\RequirePackage{amsmath}
\RequirePackage{amssymb}
\RequirePackage{amsthm}
\RequirePackage{microtype}
\RequirePackage[round,sort&compress,authoryear]{natbib}
\RequirePackage{booktabs}
\RequirePackage{xcolor}
\RequirePackage{caption}
\captionsetup{font=small,labelfont=bf}
\RequirePackage[colorlinks=true,linkcolor=blue!50!black,citecolor=blue!50!black,urlcolor=blue!50!black]{hyperref}
\RequirePackage{url}

%% Default bibliography style — consumers can override with their own
%% \bibliographystyle{...} in main.tex.
\AtBeginDocument{%
  \ifx\@bibstyle\@empty\bibliographystyle{plainnat}\fi%
}

%% Anonymous-mode author block --------------------------------------------
\ifanvil@anonymous
  \renewcommand{\@maketitle}{%
    \begin{center}%
      {\LARGE\bfseries\@title\par}%
      \vskip 1em%
      {\large Author Name(s) Withheld\par}%
      {\itshape Affiliation Withheld\par}%
      \vskip 0.5em%
      {\small\@date\par}%
    \end{center}%
  }%
\fi

%% Abstract formatting -----------------------------------------------------
\renewenvironment{abstract}%
  {\begin{center}\bfseries Abstract\end{center}%
   \begin{quote}\small}%
  {\end{quote}}

%% Section spacing — slightly tighter than article defaults
\renewcommand{\section}{\@startsection{section}{1}{\z@}%
  {-2.5ex \@plus -1ex \@minus -.2ex}%
  {1.5ex \@plus.2ex}%
  {\normalfont\large\bfseries}}
\renewcommand{\subsection}{\@startsection{subsection}{2}{\z@}%
  {-2.0ex \@plus -1ex \@minus -.2ex}%
  {1.0ex \@plus.2ex}%
  {\normalfont\normalsize\bfseries}}

%% End of class ------------------------------------------------------------
\endinput
