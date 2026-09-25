"""Canonical immutable launch contract shared across agentctl process boundaries."""
from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import cast


RUNTIME_LAUNCH_SCHEMA = "agentctl-runtime-launch/v1"
RUNTIME_CONTROL_SCHEMA = "agentctl-runtime-control/v1"
OUTER_SESSION_CONTROL = "outer-session"
STANDALONE_CONTROL = "standalone"
# Decode-edge state for the short-lived V2 format, whose token did not say
# whether the row came from the outer session manager or the compatibility CLI.
# It is never emitted by a current writer.
LEGACY_UNDETERMINED_CONTROL = "legacy-undetermined"


@dataclass(frozen=True)
class RuntimeControl:
    """The one authority allowed to mutate a persistent runtime generation."""

    kind: str
    generation: str

    @staticmethod
    def _validate_generation(generation: str) -> str:
        if re.fullmatch(r"[a-z0-9-]{1,80}", generation) is None:
            raise ValueError("runtime control generation has an invalid shape")
        return generation

    @classmethod
    def outer_session(cls, token: str) -> "RuntimeControl":
        return cls(
            kind=OUTER_SESSION_CONTROL,
            generation=cls._validate_generation(token),
        )

    @classmethod
    def standalone(cls, generation: str) -> "RuntimeControl":
        return cls(
            kind=STANDALONE_CONTROL,
            generation=cls._validate_generation(generation),
        )

    @classmethod
    def legacy_undetermined(cls, generation: str) -> "RuntimeControl":
        return cls(
            kind=LEGACY_UNDETERMINED_CONTROL,
            generation=cls._validate_generation(generation),
        )

    @classmethod
    def from_document(cls, value: object) -> "RuntimeControl":
        if not isinstance(value, dict):
            raise ValueError("runtime control must be an object")
        kind = value.get("kind")
        if set(value) != {"schema", "kind", "generation"}:
            raise ValueError("runtime control has invalid fields")
        if value.get("schema") != RUNTIME_CONTROL_SCHEMA:
            raise ValueError("runtime control has an unsupported schema")
        generation = value.get("generation")
        if not isinstance(generation, str):
            raise ValueError("runtime control has no generation")
        if kind == OUTER_SESSION_CONTROL:
            return cls.outer_session(generation)
        if kind == STANDALONE_CONTROL:
            return cls.standalone(generation)
        raise ValueError("runtime control has an unsupported kind")

    def to_document(self) -> dict[str, object]:
        if self.kind == OUTER_SESSION_CONTROL:
            type(self).outer_session(self.generation)
        elif self.kind == STANDALONE_CONTROL:
            type(self).standalone(self.generation)
        else:
            raise ValueError("runtime control is invalid")
        return {
            "schema": RUNTIME_CONTROL_SCHEMA,
            "kind": self.kind,
            "generation": self.generation,
        }


@dataclass(frozen=True)
class RuntimeLaunchContract:
    """Normalized immutable headless launch intent and its checked identifier."""

    cwd: str
    harness: str
    model: str | None
    backend: str
    mode: str
    harness_args: tuple[str, ...]
    permission_mode: str
    runtime_home: str

    @classmethod
    def create(
        cls, *, cwd: str, harness: str, model: str | None, backend: str,
        mode: str, harness_args: Sequence[str], permission_mode: str,
        runtime_home: Path | str,
    ) -> "RuntimeLaunchContract":
        return cls(
            cwd=str(Path(cwd).expanduser().resolve()),
            harness=harness,
            model=model,
            backend=backend,
            mode=mode,
            harness_args=tuple(harness_args),
            permission_mode=permission_mode,
            runtime_home=str(Path(runtime_home).expanduser().resolve()),
        )

    @classmethod
    def from_document(cls, value: object) -> "RuntimeLaunchContract":
        fields = {
            "schema", "cwd", "harness", "model", "backend", "mode",
            "harness_args", "permission_mode", "runtime_home", "fingerprint",
        }
        if not isinstance(value, dict) or set(value) != fields:
            raise ValueError("owner launch has an invalid field set")
        if value.get("schema") != RUNTIME_LAUNCH_SCHEMA:
            raise ValueError("owner launch has an unsupported schema")
        strings = [
            value.get(key) for key in (
                "cwd", "harness", "backend", "mode", "permission_mode",
                "runtime_home",
            )
        ]
        model = value.get("model")
        arguments = value.get("harness_args")
        if (any(not isinstance(item, str) or not item or "\0" in item
                for item in strings)
                or (model is not None
                    and (not isinstance(model, str) or not model or "\0" in model))
                or not isinstance(arguments, list)
                or any(not isinstance(item, str) or not item or "\0" in item
                       for item in arguments)):
            raise ValueError("owner launch contains invalid values")
        contract = cls.create(
            cwd=cast(str, value["cwd"]),
            harness=cast(str, value["harness"]),
            model=cast(str | None, model),
            backend=cast(str, value["backend"]),
            mode=cast(str, value["mode"]),
            harness_args=cast(list[str], arguments),
            permission_mode=cast(str, value["permission_mode"]),
            runtime_home=cast(str, value["runtime_home"]),
        )
        if value.get("fingerprint") != contract.fingerprint():
            raise ValueError("owner launch fingerprint disagrees with its fields")
        return contract

    def fingerprint(self) -> str:
        return runtime_launch_fingerprint(
            cwd=self.cwd,
            harness=self.harness,
            model=self.model,
            backend=self.backend,
            mode=self.mode,
            harness_args=self.harness_args,
            permission_mode=self.permission_mode,
            runtime_home=self.runtime_home,
        )

    def to_document(self) -> dict[str, object]:
        document = runtime_launch_document(
            cwd=self.cwd,
            harness=self.harness,
            model=self.model,
            backend=self.backend,
            mode=self.mode,
            harness_args=self.harness_args,
            permission_mode=self.permission_mode,
            runtime_home=self.runtime_home,
        )
        document["fingerprint"] = self.fingerprint()
        return document


def runtime_launch_document(
    *, cwd: str, harness: str, model: str | None, backend: str, mode: str,
    harness_args: Sequence[str], permission_mode: str, runtime_home: Path | str,
) -> dict[str, object]:
    """Normalize the immutable headless launch fields into one tagged document."""
    return {
        "schema": RUNTIME_LAUNCH_SCHEMA,
        "cwd": str(Path(cwd).expanduser().resolve()),
        "harness": harness,
        "model": model,
        "backend": backend,
        "mode": mode,
        "harness_args": list(harness_args),
        "permission_mode": permission_mode,
        "runtime_home": str(Path(runtime_home).expanduser().resolve()),
    }


def runtime_launch_fingerprint(
    *, cwd: str, harness: str, model: str | None, backend: str, mode: str,
    harness_args: Sequence[str], permission_mode: str, runtime_home: Path | str,
) -> str:
    """Hash exactly the normalized immutable headless launch contract."""
    document = runtime_launch_document(
        cwd=cwd,
        harness=harness,
        model=model,
        backend=backend,
        mode=mode,
        harness_args=harness_args,
        permission_mode=permission_mode,
        runtime_home=runtime_home,
    )
    encoded = json.dumps(
        document, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
        allow_nan=False,
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()
