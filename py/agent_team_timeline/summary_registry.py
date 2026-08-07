"""Registry and provenance vocabulary for every model-backed computation.

The registry is deliberately data-only. Prompt builders, cache runners, and archive writers import
the same immutable specifications so a new prompt contract cannot exist without a visible version
and changelog entry.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Final

from agent_team_timeline.archive import JsonValue


PHASE_STYLE: Final = "phase"
TECHNICAL_ROLLUP_STYLE: Final = "technical-rollup"
PLAIN_LANGUAGE_ROLLUP_STYLE: Final = "plain-language-rollup"
PROJECT_OVERVIEW_STYLE: Final = "project-overview"
GLOSSARY_DEFINITION_STYLE: Final = "glossary-definition"
AGENT_LIFETIME_STYLE: Final = "agent-lifetime"


@dataclass(frozen=True, slots=True)
class SummarizerChange:
    """One user-visible prompt/output contract revision."""

    version: int
    prompt_version: str
    change: str

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return deterministic registry metadata."""

        return {
            "version": self.version,
            "prompt_version": self.prompt_version,
            "change": self.change,
        }


@dataclass(frozen=True, slots=True)
class SummarizerSpec:
    """The current contract and complete known version history for one model task."""

    summarizer_id: str
    summary_style: str
    current_version: int
    prompt_version: str
    output_schema_version: int
    description: str
    input_fields: tuple[str, ...]
    output_fields: tuple[str, ...]
    granularities: tuple[str, ...]
    changelog: tuple[SummarizerChange, ...]

    def __post_init__(self) -> None:
        if not self.summarizer_id or not self.summary_style:
            raise ValueError("summarizer ID and style must not be empty")
        if self.current_version <= 0 or self.output_schema_version <= 0:
            raise ValueError("summarizer versions must be positive")
        if not self.changelog:
            raise ValueError(f"summarizer {self.summarizer_id!r} has no changelog")
        versions = tuple(item.version for item in self.changelog)
        if versions != tuple(sorted(set(versions))):
            raise ValueError(
                f"summarizer {self.summarizer_id!r} changelog versions are not unique and ordered"
            )
        current = self.changelog[-1]
        if (
            current.version != self.current_version
            or current.prompt_version != self.prompt_version
        ):
            raise ValueError(
                f"summarizer {self.summarizer_id!r} current contract differs from its changelog"
            )

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return deterministic registry metadata for archive inspection."""

        return {
            "summarizer_id": self.summarizer_id,
            "summary_style": self.summary_style,
            "current_version": self.current_version,
            "prompt_version": self.prompt_version,
            "output_schema_version": self.output_schema_version,
            "description": self.description,
            "input_fields": list(self.input_fields),
            "output_fields": list(self.output_fields),
            "granularities": list(self.granularities),
            "changelog": [item.to_json_obj() for item in self.changelog],
        }


@dataclass(frozen=True, slots=True)
class ContextComponent:
    """Availability of one optional context channel at generation time."""

    name: str
    requested: int
    provided: int
    unit: str

    def __post_init__(self) -> None:
        if not self.name or not self.unit:
            raise ValueError("context component name and unit must not be empty")
        if self.requested < 0 or self.provided < 0:
            raise ValueError("context component counts must not be negative")
        if self.provided > self.requested:
            raise ValueError("provided context must not exceed requested context")

    @property
    def coverage_basis_points(self) -> int:
        """Return this channel's coverage from 0 through 10,000 basis points."""

        if self.requested == 0:
            return 10_000
        return (self.provided * 10_000) // self.requested

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return deterministic context provenance."""

        return {
            "name": self.name,
            "requested": self.requested,
            "provided": self.provided,
            "unit": self.unit,
            "coverage_basis_points": self.coverage_basis_points,
        }


@dataclass(frozen=True, slots=True)
class ContextCoverage:
    """Comparable context completeness plus chronological frontier provenance."""

    components: tuple[ContextComponent, ...] = ()
    frontier_status: str = "not-applicable"
    predecessor_keys: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if self.frontier_status not in {
            "not-applicable",
            "project-start",
            "contiguous-extension",
            "isolated-backfill",
            "unknown-legacy",
        }:
            raise ValueError(f"unsupported frontier status {self.frontier_status!r}")
        names = tuple(item.name for item in self.components)
        if len(names) != len(set(names)):
            raise ValueError("context component names must be unique")

    @property
    def coverage_basis_points(self) -> int:
        """Average optional-channel completeness without mixing unlike raw units."""

        if not self.components:
            return 10_000
        return sum(item.coverage_basis_points for item in self.components) // len(
            self.components
        )

    @property
    def coverage_percent(self) -> int:
        """Return a simple whole-number context coverage score."""

        return (self.coverage_basis_points + 50) // 100

    @property
    def missing_percent(self) -> int:
        """Return the complementary whole-number missing-context score."""

        return 100 - self.coverage_percent

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return deterministic context and frontier metadata."""

        return {
            "coverage_basis_points": self.coverage_basis_points,
            "coverage_percent": self.coverage_percent,
            "missing_percent": self.missing_percent,
            "frontier_status": self.frontier_status,
            "predecessor_keys": list(self.predecessor_keys),
            "components": [item.to_json_obj() for item in self.components],
        }


PHASE_SUMMARIZER: Final = SummarizerSpec(
    summarizer_id="phase-work-summary",
    summary_style=PHASE_STYLE,
    current_version=1,
    prompt_version="agent-team-timeline-summary-v1",
    output_schema_version=1,
    description="Phrase, hover paragraph, and chronological work bullets for one agent phase.",
    input_fields=(
        "bounded phase transcript",
        "ancestor transcript scroll-back",
        "chronologically available glossary",
        "phase activity statistics",
    ),
    output_fields=("phrase", "paragraph", "work_summary"),
    granularities=("phase",),
    changelog=(
        SummarizerChange(
            1,
            "agent-team-timeline-summary-v1",
            "Initial substantive phase summary with three display resolutions.",
        ),
    ),
)

TECHNICAL_ROLLUP_SUMMARIZER: Final = SummarizerSpec(
    summarizer_id="technical-rollup",
    summary_style=TECHNICAL_ROLLUP_STYLE,
    current_version=2,
    prompt_version="agent-team-timeline-technical-rollup-v2",
    output_schema_version=1,
    description="Content-led technical rollup over lower-level cached summaries.",
    input_fields=(
        "lower-level summaries or uncovered phase summaries",
        "up to ten prior same-level technical summaries",
        "chronologically available glossary",
        "aggregate activity statistics",
    ),
    output_fields=("phrase", "paragraph", "work_summary"),
    granularities=("daily", "weekly", "monthly", "quarterly"),
    changelog=(
        SummarizerChange(
            1,
            "agent-team-timeline-technical-rollup-v1",
            "Initial calendar super-summary.",
        ),
        SummarizerChange(
            2,
            "agent-team-timeline-technical-rollup-v2",
            "Require content before work-management identifiers and expand opaque shorthand.",
        ),
    ),
)

PLAIN_LANGUAGE_ROLLUP_SUMMARIZER: Final = SummarizerSpec(
    summarizer_id="plain-language-rollup",
    summary_style=PLAIN_LANGUAGE_ROLLUP_STYLE,
    current_version=2,
    prompt_version="agent-team-timeline-plain-rollup-v2",
    output_schema_version=1,
    description="Newcomer-oriented rollup independent from the technical summary.",
    input_fields=(
        "lower-level plain-language summaries or uncovered phase summaries",
        "up to ten prior same-level plain-language summaries",
        "project overview",
        "supported chronologically available glossary definitions",
        "aggregate activity statistics",
    ),
    output_fields=("phrase", "paragraph", "work_summary"),
    granularities=("daily", "weekly", "monthly", "quarterly"),
    changelog=(
        SummarizerChange(
            1,
            "agent-team-timeline-plain-rollup-v1",
            "Initial separate plain-language calendar summary.",
        ),
        SummarizerChange(
            2,
            "agent-team-timeline-plain-rollup-v2",
            "Ground newcomer explanations in the project overview and verified glossary.",
        ),
    ),
)

PROJECT_OVERVIEW_SUMMARIZER: Final = SummarizerSpec(
    summarizer_id="project-overview",
    summary_style=PROJECT_OVERVIEW_STYLE,
    current_version=2,
    prompt_version="agent-team-timeline-project-overview-v2",
    output_schema_version=1,
    description="Durable newcomer overview from bounded early coordinator evidence.",
    input_fields=("up to 48,000 characters of early root transcript",),
    output_fields=("support_status", "paragraph"),
    granularities=("project",),
    changelog=(
        SummarizerChange(
            1,
            "agent-team-timeline-project-overview-v1",
            "Initial project knowledge summary.",
        ),
        SummarizerChange(
            2,
            "agent-team-timeline-project-overview-v2",
            "Freeze evidence frontiers and require explicit insufficient-evidence results.",
        ),
    ),
)

GLOSSARY_DEFINITION_SUMMARIZER: Final = SummarizerSpec(
    summarizer_id="glossary-definition",
    summary_style=GLOSSARY_DEFINITION_STYLE,
    current_version=2,
    prompt_version="agent-team-timeline-glossary-definition-v2",
    output_schema_version=1,
    description="Evidence-bounded newcomer definition for one deterministic glossary term.",
    input_fields=(
        "exact glossary spelling",
        "up to six retained source occurrences",
        "frozen project overview",
    ),
    output_fields=("support_status", "definition"),
    granularities=("term",),
    changelog=(
        SummarizerChange(
            1,
            "agent-team-timeline-glossary-definition-v1",
            "Initial model-backed glossary definition.",
        ),
        SummarizerChange(
            2,
            "agent-team-timeline-glossary-definition-v2",
            "Bind definitions to frozen occurrences and reject unsupported inference.",
        ),
    ),
)

AGENT_LIFETIME_SUMMARIZER: Final = SummarizerSpec(
    summarizer_id="agent-lifetime",
    summary_style=AGENT_LIFETIME_STYLE,
    current_version=2,
    prompt_version="agent-team-timeline-agent-name-v2",
    output_schema_version=2,
    description="Hindsight short name and lifetime paragraph for one agent identity.",
    input_fields=(
        "official path, coordinator nickname, role, depth, and parent path",
        "ancestor transcript scroll-back",
        "all available phase work summaries for the agent lifetime",
    ),
    output_fields=("short_name", "rationale", "lifetime_summary"),
    granularities=("agent-lifetime",),
    changelog=(
        SummarizerChange(
            1,
            "agent-team-timeline-agent-name-v1",
            "Initial hindsight short name.",
        ),
        SummarizerChange(
            2,
            "agent-team-timeline-agent-name-v2",
            "Add the substantive lifetime summary used by agent hover cards.",
        ),
    ),
)

SUMMARIZER_REGISTRY: Final = (
    PHASE_SUMMARIZER,
    AGENT_LIFETIME_SUMMARIZER,
    PROJECT_OVERVIEW_SUMMARIZER,
    GLOSSARY_DEFINITION_SUMMARIZER,
    TECHNICAL_ROLLUP_SUMMARIZER,
    PLAIN_LANGUAGE_ROLLUP_SUMMARIZER,
)

_BY_STYLE: Final = {item.summary_style: item for item in SUMMARIZER_REGISTRY}

if len(_BY_STYLE) != len(SUMMARIZER_REGISTRY):
    raise ValueError("summarizer registry contains duplicate styles")
if len({item.summarizer_id for item in SUMMARIZER_REGISTRY}) != len(
    SUMMARIZER_REGISTRY
):
    raise ValueError("summarizer registry contains duplicate IDs")


def summarizer_for_style(summary_style: str) -> SummarizerSpec:
    """Return the registered contract for an internal summary style."""

    try:
        return _BY_STYLE[summary_style]
    except KeyError as error:
        raise ValueError(f"unregistered summary style {summary_style!r}") from error


def registry_json_obj() -> dict[str, JsonValue]:
    """Return the complete deterministic registry for docs and archive manifests."""

    return {
        "schema_version": 1,
        "summarizers": [item.to_json_obj() for item in SUMMARIZER_REGISTRY],
    }


__all__ = [
    "AGENT_LIFETIME_STYLE",
    "AGENT_LIFETIME_SUMMARIZER",
    "ContextComponent",
    "ContextCoverage",
    "GLOSSARY_DEFINITION_STYLE",
    "GLOSSARY_DEFINITION_SUMMARIZER",
    "PHASE_STYLE",
    "PHASE_SUMMARIZER",
    "PLAIN_LANGUAGE_ROLLUP_STYLE",
    "PLAIN_LANGUAGE_ROLLUP_SUMMARIZER",
    "PROJECT_OVERVIEW_STYLE",
    "PROJECT_OVERVIEW_SUMMARIZER",
    "SUMMARIZER_REGISTRY",
    "SummarizerChange",
    "SummarizerSpec",
    "TECHNICAL_ROLLUP_STYLE",
    "TECHNICAL_ROLLUP_SUMMARIZER",
    "registry_json_obj",
    "summarizer_for_style",
]
