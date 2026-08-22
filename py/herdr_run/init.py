"""Writing a project's ``.herdr-run.yaml`` from the annotated template.

The template carries EVERY knob, each set to the value the tool would use anyway, so a project can
adopt the file and change nothing. That is deliberate: the reference for what is configurable is
the file the tool writes into the project, not a block of documentation that restates it and then
drifts from it.
"""

from __future__ import annotations

import os

from herdr_run.config import CONFIG_FILENAMES
from herdr_run.errors import ConfigError

__all__ = ["config_template", "write_config_template"]

#: Package resource holding the complete annotated ``.herdr-run.yaml``.
TEMPLATE_RESOURCE = "config_template.yaml"


def config_template() -> str:
    """Return the annotated ``.herdr-run.yaml`` this installation ships."""
    try:
        from importlib.resources import files

        return (files("herdr_run") / TEMPLATE_RESOURCE).read_text(encoding="utf-8")
    except (OSError, ModuleNotFoundError) as exc:  # pragma: no cover - damaged installation
        raise ConfigError(
            "the packaged configuration template is not available in this installation. "
            "Repair this herdr-run installation."
        ) from exc


def write_config_template(directory: str, *, force: bool) -> str:
    """Write the annotated configuration template into ``directory``; return the path written.

    Refuses to overwrite an existing configuration unless ``force``. It also refuses when only the
    ``.yml`` spelling exists, because the ``.yaml`` this would write silently takes precedence over
    it: a caller who ends up with two files and no warning has been given a puzzle, not a config.
    """
    destination = os.path.join(directory, CONFIG_FILENAMES[0])
    if not force:
        for filename in CONFIG_FILENAMES:
            existing = os.path.join(directory, filename)
            if not os.path.exists(existing):
                continue
            precedence = (
                ""
                if filename == CONFIG_FILENAMES[0]
                else f", and the {CONFIG_FILENAMES[0]} this would write takes precedence over it"
            )
            raise ConfigError(
                f"{existing} already exists{precedence}; pass --force to overwrite"
            )
    text = config_template()
    # "x" without --force, so a file created between the check above and this open is still not
    # clobbered; the check exists to produce the useful message, not to be the guard.
    try:
        with open(destination, "w" if force else "x", encoding="utf-8") as handle:
            handle.write(text)
    except OSError as exc:
        raise ConfigError(f"cannot write {destination}: {exc}") from exc
    return destination
