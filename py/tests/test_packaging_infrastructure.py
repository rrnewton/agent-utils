from __future__ import annotations

import importlib.util
import os
import re
import sys
import zipfile
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
    item = docs.Render("dagrun", "README", "python", "out/README.md")

    errors = docs._lint(item, "{{UNKNOWN}}\nRust\ntick-hub\n")

    assert any("unexpanded template syntax" in error for error in errors)
    assert any("other implementation language" in error for error in errors)
    assert any("sibling package" in error for error in errors)


def test_embed_check_reports_both_staleness_and_lint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    docs = _load_script("embed_userguides")
    item = docs.Render("dagrun", "README", "python", "out/README.md")
    template = tmp_path / item.template
    fragment = tmp_path / item.fragment
    destination = tmp_path / item.destination
    template.parent.mkdir(parents=True)
    fragment.parent.mkdir(parents=True)
    destination.parent.mkdir(parents=True)
    template.write_text("# dagrun\n\n{{DISTRIBUTION}}\n", encoding="utf-8")
    fragment.write_text("Install this distribution.\n", encoding="utf-8")
    destination.write_text("{{UNKNOWN}}\ntick-hub\n", encoding="utf-8")
    monkeypatch.setattr(docs, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(docs, "RENDERS", (item,))
    monkeypatch.setattr(docs, "STANDALONE_DOCUMENTS", ())
    monkeypatch.setattr(docs, "PACKAGE_LINKS", ())

    stale, lint_errors = docs.check()

    assert stale == [item.destination]
    assert any("unexpanded template syntax" in error for error in lint_errors)
    assert any("sibling package" in error for error in lint_errors)


def test_embed_generate_prevalidates_and_writes_only_changed_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    docs = _load_script("embed_userguides")
    first = docs.Render("dagrun", "README", "python", "out/README.md")
    second = docs.Render("dagrun", "USER_GUIDE", "python", "out/USER_GUIDE.md")
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
    monkeypatch.setattr(docs, "STANDALONE_DOCUMENTS", ())
    monkeypatch.setattr(docs, "PACKAGE_LINKS", ())

    with pytest.raises(FileNotFoundError, match="fragment missing"):
        docs.generate()

    assert first_destination.read_text(encoding="utf-8") == "leave me alone\n"
    second_fragment = tmp_path / second.fragment
    second_fragment.parent.mkdir(parents=True, exist_ok=True)
    second_fragment.write_text("also valid\n", encoding="utf-8")
    assert docs.generate() == [first.destination, second.destination]
    assert first_destination.read_text(encoding="utf-8") == "valid\n"

    old_mtime_ns = 1_600_000_000_000_000_000
    os.utime(first_destination, ns=(old_mtime_ns, old_mtime_ns))

    assert docs.generate() == []
    assert first_destination.stat().st_mtime_ns == old_mtime_ns

    first_destination.write_text("stale\n", encoding="utf-8")
    assert docs.generate() == [first.destination]
    assert first_destination.read_text(encoding="utf-8") == "valid\n"


def test_embed_check_rejects_regular_copy_and_wrong_link_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    docs = _load_script("embed_userguides")
    link = docs.PackageLink("package/README.md", "common/README.md")
    wanted = tmp_path / link.target
    destination = tmp_path / link.destination
    wanted.parent.mkdir(parents=True)
    destination.parent.mkdir(parents=True)
    wanted.write_text("authoritative\n", encoding="utf-8")
    destination.write_text("authoritative\n", encoding="utf-8")
    monkeypatch.setattr(docs, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(docs, "RENDERS", ())
    monkeypatch.setattr(docs, "STANDALONE_DOCUMENTS", ())
    monkeypatch.setattr(docs, "PACKAGE_LINKS", (link,))

    stale, lint_errors = docs.check()
    assert stale == [link.destination]
    assert lint_errors == []

    destination.unlink()
    destination.symlink_to("../wrong/README.md")
    stale, _ = docs.check()
    assert stale == [link.destination]

    destination.unlink()
    destination.symlink_to(link.relative_target)
    assert docs.check() == ([], [])


def test_package_docs_and_licenses_are_authoritative_links() -> None:
    docs = _load_script("embed_userguides")

    assert len(docs.PACKAGE_LINKS) == 39
    for link in docs.PACKAGE_LINKS:
        destination = REPO_ROOT / link.destination
        assert destination.is_symlink(), link.destination
        assert docs._link_is_current(link), link.destination


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


def test_python_doc_lint_exemption_cannot_hide_a_later_foreign_term() -> None:
    python_check = _load_script("check_python_packages")
    project = next(
        project for project in python_check.PROJECTS if "cargo" in project.doc_term_exemptions
    )

    errors = python_check._doc_violations(
        project,
        "Cargo is supported as a target program. Install the unrelated Rust implementation.",
    )

    assert "foreign-language term 'Rust'" in errors
    assert "foreign-language term 'Cargo'" not in errors


def test_python_sibling_dependency_requires_a_project_local_exemption() -> None:
    python_check = _load_script("check_python_packages")
    parallel = next(
        project
        for project in python_check.PROJECTS
        if project.distribution == "parallel-experiment-runner"
    )
    dagrun_requirement = {"dagrun"}

    assert not python_check._unexpected_sibling_requirements(
        parallel, dagrun_requirement
    )
    assert python_check._unexpected_sibling_requirements(
        python_check.PROJECTS[0], {"tick-hub"}
    ) == ["tick-hub"]


def test_every_declared_markdown_resource_is_standalone() -> None:
    python_check = _load_script("check_python_packages")

    for project in python_check.PROJECTS:
        source = python_check.PY_ROOT / project.directory
        # Reading the declared resources is the artifact checker's common source/sdist/wheel gate.
        python_check._source_resources(project, source)


def test_secondary_markdown_resource_gets_the_standalone_lint(tmp_path: Path) -> None:
    python_check = _load_script("check_python_packages")
    project = python_check.Project(
        directory="demo",
        distribution="demo",
        package="demo",
        commands=("demo",),
        resources=("AGENT_USER_GUIDE.md",),
        required_dependencies=(),
    )
    (tmp_path / "AGENT_USER_GUIDE.md").write_text(
        "Use the DeepScry workspace.\n", encoding="utf-8"
    )

    with pytest.raises(
        python_check.CheckError,
        match=r"AGENT_USER_GUIDE\.md is not standalone: unrelated project 'DeepScry'",
    ):
        python_check._source_resources(project, tmp_path)


def test_unexpected_package_members_reject_undeclared_documentation() -> None:
    python_check = _load_script("check_python_packages")

    unexpected = python_check._unexpected_package_members(
        {
            "demo/__init__.py",
            "demo/README.md",
            "demo/ARCHITECTURE.md",
            "demo/static/",
        },
        "demo/",
        {"demo/__init__.py"},
        ("README.md",),
    )

    assert unexpected == ["demo/ARCHITECTURE.md"]


def test_wheel_rejects_present_but_corrupted_declared_resource(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    python_check = _load_script("check_python_packages")
    repo_root = tmp_path / "repo"
    py_root = repo_root / "py"
    source = py_root / "asset_demo"
    (source / "static").mkdir(parents=True)

    readme = "# Asset demo\n"
    userguide = "# Asset demo user guide\n"
    (repo_root / "LICENSE").write_text("test license\n", encoding="utf-8")
    (source / "__init__.py").write_text('"""Asset demo package."""\n', encoding="utf-8")
    (source / "README.md").write_text(readme, encoding="utf-8")
    (source / "USER_GUIDE.md").write_text(userguide, encoding="utf-8")
    (source / "py.typed").write_bytes(b"")
    trusted_core = b"globalThis.TimelineCore = {trusted: true};\n"
    (source / "static" / "timeline-core.js").write_bytes(trusted_core)

    project = python_check.Project(
        directory="asset_demo",
        distribution="asset-demo",
        package="asset_demo",
        commands=("asset-demo",),
        resources=("README.md", "USER_GUIDE.md", "py.typed", "static/timeline-core.js"),
        required_dependencies=(),
    )
    monkeypatch.setattr(python_check, "REPO_ROOT", repo_root)
    monkeypatch.setattr(python_check, "PY_ROOT", py_root)
    monkeypatch.setattr(python_check, "PROJECTS", (project,))

    wheel = tmp_path / "asset_demo-1.0-py3-none-any.whl"
    dist_info = "asset_demo-1.0.dist-info"
    package_prefix = "asset_demo/"
    with zipfile.ZipFile(wheel, mode="w") as archive:
        archive.writestr(
            f"{dist_info}/METADATA",
            "Metadata-Version: 2.4\n"
            "Name: asset-demo\n"
            "Version: 1.0\n"
            "Requires-Python: >=3.10\n"
            "Description-Content-Type: text/markdown\n"
            f"\n{readme}",
        )
        archive.writestr(
            f"{dist_info}/entry_points.txt",
            "[console_scripts]\nasset-demo = asset_demo.cli:main\n",
        )
        archive.writestr(f"{dist_info}/licenses/LICENSE", b"test license\n")
        archive.writestr(f"{package_prefix}__init__.py", b'"""Asset demo package."""\n')
        archive.writestr(f"{package_prefix}README.md", readme)
        archive.writestr(f"{package_prefix}USER_GUIDE.md", userguide)
        archive.writestr(f"{package_prefix}py.typed", b"")
        # This member's presence satisfied the old checker even though its payload is corrupt.
        archive.writestr(
            f"{package_prefix}static/timeline-core.js",
            b"globalThis.TimelineCore = {trusted: false};\n",
        )

    with zipfile.ZipFile(wheel) as archive:
        assert f"{package_prefix}static/timeline-core.js" in archive.namelist()
    with pytest.raises(
        python_check.CheckError,
        match=r"wheel static/timeline-core\.js differs from its authoritative source",
    ):
        python_check._inspect_wheel(project, wheel)


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


def test_dagrun_rust_dependency_snippets_match_the_published_minor_version() -> None:
    manifest = (REPO_ROOT / "rs" / "dagrun" / "Cargo.toml").read_text(encoding="utf-8")
    matched = re.search(r'^version = "(\d+\.\d+)\.\d+"$', manifest, re.MULTILINE)
    assert matched is not None
    dependency = f'dagrun = "{matched.group(1)}"'
    for name in ("README.md", "USER_GUIDE.md"):
        fragment = (
            REPO_ROOT / "common" / "docs" / "dagrun" / "fragments" / "rust" / name
        ).read_text(encoding="utf-8")
        assert dependency in fragment


def test_package_indexes_name_real_check_commands() -> None:
    python_index = (REPO_ROOT / "py" / "README.md").read_text(encoding="utf-8")
    rust_index = (REPO_ROOT / "rs" / "README.md").read_text(encoding="utf-8")
    makefile = (REPO_ROOT / "Makefile").read_text(encoding="utf-8")

    assert "make check-python-packages" in python_index
    assert "make check-rust-packages" in rust_index
    assert "check-python-packages:" in makefile
    assert "check-rust-packages:" in makefile
    assert "python" not in rust_index.lower()
