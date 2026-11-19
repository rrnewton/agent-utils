---
title: 'wrkslots-pathlib-uninitialised-path: PosixPath objects missing their private slots (_hash, _str) appear in loaded gate shards'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- tests
depends_on: {}
created_at: 2026-11-19T21:40:00+00:00
updated_at: 2026-11-19T21:40:00+00:00
---

# Description

[watermelon-full-preview] In loaded gate runs, two image tests failed with a PosixPath whose private attributes were never initialised: pathlib tried `self._hash`, then `_str_normcase_cached`, then `_str`, then `_drv`, each raising AttributeError. A Path in that state was built without `__init__` running (pickling, copy, or a monkeypatch across Python versions are the usual routes), or its `__dict__` was replaced after construction. It is not ordinary load behaviour: both tests pass alone on the same tree and on github/main. Failing tests (wrkslots non-lifecycle shards, 2026-11-19 gate on the B2 census stack):

- py/wrkslots/tests/test_worktrees_image.py::test_remove_refuses_when_the_bind_unmount_fails_and_a_retry_finishes (shard 2 of 16)
- py/wrkslots/tests/test_worktrees_image.py::test_validate_mount_fence_refuses_when_the_unmount_fails (shard 13 of 16)

Traceback (first frames, identical shape in both):

```
self = PosixPath('<eden-mounts>/<machine>-<id>/validate/<slot>')
    def __hash__(self):
        try:
>           return self._hash
E           AttributeError: 'PosixPath' object has no attribute '_hash'
pathlib.py:526: AttributeError
    @property
    def _str_normcase(self):
        try:
>           return self._str_normcase_cached
E           AttributeError: 'PosixPath' object has no attribute '_str_normcase_cached'. Did you mean: '_parts_normcase_cached'?
pathlib.py:488: AttributeError
    def __str__(self):
        try:
>           return self._str
E           AttributeError: 'PosixPath' object has no attribute '_str'
pathlib.py:441: AttributeError
```

The path in the traceback is a real Eden mount path read back from state, so the suspect is wherever a stored path is reconstructed: JSON round-trip, a test double that fabricates Path objects, or a cross-version unpickle in the shard runner.

# Acceptance Criteria

A reproduction names the construction route that yields an uninitialised PosixPath (or proves each candidate route safe), the route is fixed or the fabrication is replaced with a real Path, and the two named tests pass in a loaded shard run, not only alone.

# Notes

Filed from the B2 census gate triage at coordb's request. Both tests passed alone on the gated tree and on github/main (solo runs 2026-11-19), so this tracks a load-dependent fabrication, not a product regression.
