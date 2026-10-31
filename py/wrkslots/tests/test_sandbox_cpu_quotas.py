"""Per-slot CPU quota overrides: first prefix wins, default otherwise."""

import pytest

from wrkslots.sandbox import (
    SandboxError,
    SandboxLimits,
    effective_cpu_quota,
    limits_from_obj,
    settings_from_obj,
    settings_to_obj,
    slice_limit_properties,
)


def limits() -> SandboxLimits:
    return limits_from_obj(
        {
            "cpu_quota": "800%",
            "slot_cpu_quotas": [
                {"slot_prefix": "li2core-", "cpu_quota": "1200%"},
                {"slot_prefix": "li2", "cpu_quota": "900%"},
                {"slot_prefix": "kvm-", "cpu_quota": "2400%"},
            ],
        }
    )


def test_prefix_override_first_match_wins() -> None:
    lim = limits()
    assert effective_cpu_quota(lim, "li2core-20261024") == "1200%"
    assert effective_cpu_quota(lim, "li2adm-20261024") == "900%"
    assert effective_cpu_quota(lim, "kvm-20261012") == "2400%"


def test_default_applies_without_match() -> None:
    lim = limits()
    assert effective_cpu_quota(lim, "museops-20261028") == "800%"
    assert effective_cpu_quota(lim, None) == "800%"
    assert "CPUQuota=800%" in slice_limit_properties(lim, "museops-20261028")
    assert "CPUQuota=2400%" in slice_limit_properties(lim, "kvm-20261012")


def test_no_quota_when_unset() -> None:
    lim = limits_from_obj({})
    assert effective_cpu_quota(lim, "kvm-1") is None
    assert not any(p.startswith("CPUQuota") for p in slice_limit_properties(lim, "kvm-1"))


def test_config_round_trip() -> None:
    settings = settings_from_obj({"limits": {
        "cpu_quota": "800%",
        "slot_cpu_quotas": [{"slot_prefix": "kvm-", "cpu_quota": "2400%"}],
    }})
    obj = settings_to_obj(settings)
    limits_obj = obj["limits"]
    assert isinstance(limits_obj, dict)
    assert limits_obj["slot_cpu_quotas"] == [
        {"slot_prefix": "kvm-", "cpu_quota": "2400%"}
    ]
    assert limits_from_obj(limits_obj) == settings.limits


@pytest.mark.parametrize("bad", [
    [{"slot_prefix": "", "cpu_quota": "800%"}],
    [{"slot_prefix": "kvm-"}],
    [{"slot_prefix": "kvm-", "cpu_quota": None}],
    [{"slot_prefix": "kvm-", "cpu_quota": "lots"}],
    [{"slot_prefix": "kvm-", "cpu_quota": "800%", "extra": 1}],
    "kvm-",
])
def test_malformed_overrides_refused(bad: object) -> None:
    with pytest.raises(SandboxError):
        limits_from_obj({"slot_cpu_quotas": bad})


def test_settings_to_obj_serializes_overrides() -> None:
    settings = settings_from_obj({"limits": {
        "cpu_quota": "800%",
        "slot_cpu_quotas": [{"slot_prefix": "kvm-", "cpu_quota": "2400%"}],
    }})
    obj = settings_to_obj(settings)
    limits_obj = obj["limits"]
    assert isinstance(limits_obj, dict)
    assert limits_obj["slot_cpu_quotas"] == [
        {"slot_prefix": "kvm-", "cpu_quota": "2400%"}
    ]
