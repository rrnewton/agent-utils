from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]


def _load_script(name: str) -> ModuleType:
    path = REPO_ROOT / "scripts" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(f"_packaging_test_{name}", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_embed_lint_rejects_unknown_placeholders_and_sibling_packages() -> None:
    docs = _load_script("embed_userguides")
    item = docs.Render("safe-ci-dag-runner", "README", "python", "out/README.md")

    errors = docs._lint(item, "{{UNKNOWN}}\nRust\ntick-hub\n")

    assert any("unexpanded template syntax" in error for error in errors)
    assert any("other implementation language" in error for error in errors)
    assert any("sibling package" in error for error in errors)


def test_embed_check_reports_both_staleness_and_lint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    docs = _load_script("embed_userguides")
    item = docs.Render("safe-ci-dag-runner", "README", "python", "out/README.md")
    template = tmp_path / item.template
    fragment = tmp_path / item.fragment
    destination = tmp_path / item.destination
    template.parent.mkdir(parents=True)
    fragment.parent.mkdir(parents=True)
    destination.parent.mkdir(parents=True)
    template.write_text("# safe-ci-dag-runner\n\n{{DISTRIBUTION}}\n", encoding="utf-8")
    fragment.write_text("Install this distribution.\n", encoding="utf-8")
    destination.write_text("{{UNKNOWN}}\ntick-hub\n", encoding="utf-8")
    monkeypatch.setattr(docs, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(docs, "RENDERS", (item,))
    monkeypatch.setattr(docs, "DIRECT_COPIES", ())

    stale, lint_errors = docs.check()

    assert stale == [item.destination]
    assert any("unexpanded template syntax" in error for error in lint_errors)
    assert any("sibling package" in error for error in lint_errors)


def test_embed_prevalidates_every_input_before_writing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    docs = _load_script("embed_userguides")
    first = docs.Render("safe-ci-dag-runner", "README", "python", "out/README.md")
    second = docs.Render(
        "safe-ci-dag-runner", "USER_GUIDE", "python", "out/USER_GUIDE.md"
    )
    first_template = tmp_path / first.template
    first_fragment = tmp_path / first.fragment
    first_destination = tmp_path / first.destination
    second_template = tmp_path / second.template
    first_template.parent.mkdir(parents=True)
    first_fragment.parent.mkdir(parents=True)
    first_destination.parent.mkdir(parents=True)
    second_template.parent.mkdir(parents=True, exist_ok=True)
    first_template.write_text("{{DISTRIBUTION}}\n", encoding="utf-8")
    first_fragment.write_text("valid\n", encoding="utf-8")
    first_destination.write_text("leave me alone\n", encoding="utf-8")
    second_template.write_text("{{DISTRIBUTION}}\n", encoding="utf-8")
    monkeypatch.setattr(docs, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(docs, "RENDERS", (first, second))
    monkeypatch.setattr(docs, "DIRECT_COPIES", ())

    with pytest.raises(FileNotFoundError, match="fragment missing"):
        docs.generate()

    assert first_destination.read_text(encoding="utf-8") == "leave me alone\n"


def test_artifact_doc_linters_reject_suite_language_sibling_and_template_leaks() -> None:
    python_check = _load_script("check_python_packages")
    rust_check = _load_script("check_rust_packages")

    python_errors = python_check._doc_violations(
        python_check.PROJECTS[0], "agent-utils Rust tick-hub {{UNKNOWN}}"
    )
    rust_errors = rust_check._doc_violations(
        rust_check.CRATES[0], "agent-utils Python tick-hub {{UNKNOWN}}"
    )

    for errors in (python_errors, rust_errors):
        assert any("suite name" in error for error in errors)
        assert any("foreign" in error for error in errors)
        assert any("sibling package" in error for error in errors)
        assert any("template" in error for error in errors)


def test_public_api_docs_are_standalone_without_banning_native_package_terms() -> None:
    python_check = _load_script("check_python_packages")
    rust_check = _load_script("check_rust_packages")

    python_check._check_public_docstrings()
    rust_check._check_public_rustdoc()

    assert not python_check._doc_violations(
        python_check.PROJECTS[0], "Install this Python package from PyPI with pip."
    )
    assert not rust_check._doc_violations(
        rust_check.CRATES[0], "Install this Rust crate with Cargo."
    )


def test_package_indexes_name_real_check_commands() -> None:
    python_index = (REPO_ROOT / "py" / "README.md").read_text(encoding="utf-8")
    rust_index = (REPO_ROOT / "rs" / "README.md").read_text(encoding="utf-8")
    makefile = (REPO_ROOT / "Makefile").read_text(encoding="utf-8")

    assert "make check-python-packages" in python_index
    assert "make check-rust-packages" in rust_index
    assert "check-python-packages:" in makefile
    assert "check-rust-packages:" in makefile
    assert "python" not in rust_index.lower()
