• *the core renders more `/proc` itself:* `/proc/<pid>/cmdline`, `stat`, `auxv`, `cgroup` and `comm`, plus every process's mount table. Each container also gets a frozen `/etc/ld.so.cache`. These no longer depend on the backend or the host ([mounts](https://example.org/1), [cmdline](https://example.org/2)).
• *KVM deleted five private copies of Linux behaviour.* Random devices are recognized by device number on every backend, so `/dev/urandom` aliases no longer read host entropy ([commit](https://example.org/3)).
• *Where each backend stands* (577 test cells each, Oct 20, [scorecard](https://example.org/4)):| Backend | Two runs identical | Same output as ptrace | Whole log same as ptrace |
|---|---:|---:|---:|
| KVM | 556 | 546 | 323 |
| DBT | 550 | 498 | 0 |
| SaBRe | 313 | 300 | 0 |
| LiteInst | 303 | 290 | 0 |
| in-guest-trap | 302 | 289 | 0 |Most LiteInst and in-guest-trap gaps are named refusals (execveat alone accounts for 208). SaBRe's two runs agreed on 201 more cells, but the check that SaBRe really intercepted those calls failed.
 KVM: less CPU per system call Last week brought KVM down to one host system call per guest call.
