# Fixed-namespace installation selection fixture

This experiment uses candidate `open_fixed_namespace` entry points on the shared
Linux-test-only root reader and administrative writer. They open the literal
filesystem root of their current namespace and follow the fixed
[installation hierarchy](../../../docs/LINUX-CUSTODY-ROOT.md). They accept no
root path, directory descriptor, environment location or expected installation
ID from the requester. There is no operational deployment command or credential
admission in this experiment.

## Namespace and independent expectation

Each case has new real and decoy volumes. Administrative setup mounts the real
volume at `/var/lib/vivac` and provisions it through the fixed-namespace writer.
It independently provisions the decoy beneath `/decoy` through the existing
fixture-root API. Their writer-generated installation IDs must differ. The
missing-root case omits real provisioning but retains a complete valid decoy.
No tar archive supplies a successful binding.

The target-user reader calls only the fixed-namespace entry. An independent
observer compares its selected public installation ID with the actual setup
result and the regular binding file, not a value supplied to the reader CLI.
Environment and working-directory cases direct the helper toward the valid
decoy; selecting that decoy fails even though its structure is valid.

Opening `/` establishes selection within the current trusted deployment
namespace. It does not attest a physical host, defeat a trusted administrator
cloning an entire namespace or establish organizational authority. All ordinary
Linux custody commands remain unavailable pending the other bootstrap gates.

## Resource and privilege separation

| Role | UID/GID and capabilities | CPU | RAM | PIDs | Temporary storage |
|---|---|---:|---:|---:|---:|
| Setup, run alone | 0:0; CHOWN and DAC_READ_SEARCH only | 1 | 512 MiB | 128 | 128 MiB |
| Reader | 65534:65534; none | 0.75 | 320 MiB | 96 | 96 MiB |
| Binding mutator, only when requested | 0:0; none | 0.25 | 128 MiB | 16 | 16 MiB |

Reader and mutator together stay within 1 CPU, 448 MiB RAM, 112 PIDs and
112 MiB temporary storage. Setup must have exited before either starts.
Every container has NNP, no network, a read-only image root and no additional
swap. Public source and the selected helper volume are read-only. The reader
mounts both installation volumes read-only and has no writable state alias.
The mutator mounts only the case's real state read-write and can change its
root-owned binding as file owner without acquiring capabilities. It cannot
alter the host installation.

The runner verifies exact configurations and the simultaneous resource budget
before starting roles. Independent process checks retain pidfds, executable
identity, parent/start identity, UID/GID, NNP and every thread's capabilities.
The Rust host runs four pressure threads; final success requires its observed
exit code 0. There are no signing or custody-native processes here.

## Required cases

| Case | Required observation |
|---|---|
| `fixed-valid` | The real fixed installation opens and its retained snapshot validates |
| `environment-decoy` | HOME, VIVAC_HOME and XDG_STATE_HOME cannot select the valid decoy |
| `cwd-decoy` | A working directory inside the valid decoy cannot redirect selection |
| `missing-fixed-root` | Missing real state refuses even with valid decoy state; no fallback or repair |
| `binding-in-place` | Administrative byte mutation on the same object invalidates the held snapshot |
| `binding-replacement` | A different binding object with identical bytes invalidates the held snapshot |
| `binding-mode` | A change from public binding mode to 0600 invalidates the held snapshot |
| `mount-users` | A real additional mount at the users directory refuses with the EXDEV error category |
| `mount-state` | A real additional mount at private state refuses with the EXDEV error category |

For mutations, the reader opens and validates first, then remains alive with
the same retained object. The observer requests one bounded operation through
a case-bound nonce. The host controller starts only the owned mutator, requires
its successful exit and exact before/after object and digest properties, and
returns a matching acknowledgement before releasing the reader. The reader
must invalidate its snapshot without reopening another installation. Its
cached installation ID remains the initial one; it is not silently refreshed.
Request and acknowledgement files are fixture coordination, not a product
authority protocol or an approval mechanism.

Mount cases use Docker's
[`volume-subpath`](https://docs.docker.com/engine/storage/volumes/#mount-a-volume-subdirectory)
to mount an existing directory from the same volume over its existing location.
The reader observes the actual kernel mount boundary in mountinfo, its identity
relative to the parent mount and continuity of device/inode with setup.
The directory and bytes remain the same underlying object; the separate mount
still must fail the root reader's NO_XDEV rule. Header declarations or an
arbitrary open failure cannot stand in for these observations. No runtime
receives SYS_ADMIN or performs a mount syscall.

## Scope and reproduction

The report's candidate fixed-namespace observation is credited only after the
case's exact roles, process identities, mount/mutation properties and exits
pass. Bootstrap, operational provisioning, producer-origin, credential admission,
custody, endpoint-authority, recovery, MCP/server and lock ACK remain false.
Resident package/admission composition and trusted attended creation still need
their own implementation and native acceptance. This fixture is neither an
operational deployment nor a power-loss test.

Build a clean helper and invoke each case separately through the shared
RustWork coordinator with `--family rust` so other projects may run between cases:

```text
python run_fixed_root_probe.py --case CASE --image LOCAL_UBUNTU_IMAGE --launcher-build-volume NEW_RUST_LAUNCHER_VOLUME --reports-dir NEW_ABSOLUTE_REPORT_DIRECTORY
```

The runner observes selected RDP events, retains real failures and exit codes,
and removes only its stopped containers and temporary real/decoy volumes.
Report and helper volumes remain retained as evidence.
