## Intro

This directory contains a Python/Flask tool for viewing SCP debug dumps.
Currently, only slot state viewing is implemented.

## Status: no known log producer

The dump format comes from the upstream `consensus-service` binary, which Botho
does not carry. The Botho node has no `--scp-debug-dump` flag to produce these
files. As with the sibling [replay tool](../play/README.md), the instructions
below describe how to consume previously captured dumps, not how to capture
them from a running Botho node.

## Usage

From this directory, given a directory containing previously captured SCP JSON
slot-state dumps (subdirectories are searched recursively):

1. Create a Python virtual env: `python3 -m venv env`
1. Activate it: `. ./env/bin/activate`
1. Install dependencies: `pip install -r requirements.txt`
1. Start the web server: `python viewer.py /path/to/scp-dumps`
1. Point a browser at http://127.0.0.1:5000/ and begin exploring.
