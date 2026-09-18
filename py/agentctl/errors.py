"""Stable failure outcomes for persistent agent control."""
from __future__ import annotations

EXIT_UNAVAILABLE = 69
EXIT_BUSY = 75
EXIT_TIMEOUT = 76


class AgentCtlError(Exception):
    """Base exception for session, adapter, and delivery failures."""
    exit_code = 1


# Kept for extracted library callers while internal imports migrate.
HerdrRunError = AgentCtlError


class HerdrUnavailable(AgentCtlError):
    """The Herdr transport or a verified target cannot be reached."""
    exit_code = EXIT_UNAVAILABLE


class AgentDeliveryError(AgentCtlError):
    """Input could not be delivered under the required identity and state."""
    exit_code = EXIT_BUSY


class AgentPending(AgentDeliveryError):
    """Nothing was injected; the durable prompt remains safe to retry."""

    exit_code = EXIT_BUSY

    def __init__(self, message: str, *, message_id: str, artifact: str) -> None:
        super().__init__(message)
        self.message_id = message_id
        self.artifact = artifact
        self.outcome = "pending"


class AgentPossiblySubmitted(AgentDeliveryError):
    """Injection may have succeeded; automatic retry could duplicate a turn."""

    exit_code = EXIT_TIMEOUT

    def __init__(self, message: str, *, message_id: str, artifact: str) -> None:
        super().__init__(message)
        self.message_id = message_id
        self.artifact = artifact
        self.outcome = "possibly_submitted"
