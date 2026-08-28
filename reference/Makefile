PYTHON := .venv/bin/python

.PHONY: setup models golden check

setup:
	/opt/homebrew/bin/python3.12 -m venv .venv
	$(PYTHON) -m pip install --upgrade pip
	$(PYTHON) -m pip install -r requirements.lock.txt

models:
	$(PYTHON) export_clip_onnx.py

golden:
	$(PYTHON) generate_goldens.py
	@echo "golden regenerated — commit with a message saying WHY"

check:
	$(PYTHON) -m compileall -q *.py
	$(PYTHON) verify_goldens.py
