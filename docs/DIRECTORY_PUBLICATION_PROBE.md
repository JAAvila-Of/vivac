# Native publication identity experiments

These experiments observe native filesystem APIs on Linux and macOS. They do not
add multi-file artifact custody to vivac or change the Rust binary. A successful
job means the declared synthetic observations completed, including a deliberate
source-identity counterexample. It does not mean a bundle backend is ready.

## Directory publication

The script prepares three synthetic members and a manifest, then uses one native
rename operation to publish the complete directory. Linux uses
`renameat2(RENAME_NOREPLACE)`; macOS uses `renameatx_np(RENAME_EXCL)`. Unsupported
APIs or filesystems are reported as incomplete, without a replacing fallback.

The required observations cover occupied destinations, two competing publishers,
a held destination parent whose visible path is replaced, rejected symbolic
links, complete copied bytes, and child-process exit before or after publication.
Temporary fixtures belong to this experiment and cleanup refuses a changed run
identity or unavailable descriptor-based removal.

The source-name substitution case deliberately replaces the staging entry while
the original directory descriptor remains open. The expected observation is that
the rename publishes the replacement. The report therefore keeps
`source_identity_bound: false` and `production_ready: false`, even when every
observation matches its expectation. Opening the parent anchors resolution; it
does not bind the source name to the verified directory's inode.

These APIs protect a destination from replacement. They do not establish a
consistent source snapshot, production access bound to a project root, artifact
inspection, provenance receipts, power-loss durability, or Rust performance and
memory budgets. Process exit is not a power-loss test. No existing artifact or
project content is read, imported, executed or uploaded by either probe.

## File publication from an open descriptor

The second probe represents three synthetic members and their manifest in one
regular file. This is a complete experimental fixture, not an adopted bundle
format or a supported artifact delivery type.

Linux uses `linkat` with a generated `/proc/self/fd` alias and
`AT_SYMLINK_FOLLOW`; macOS uses `fclonefileat` with the source descriptor and
`CLONE_NOFOLLOW | CLONE_NOOWNERCOPY`. The
probe never accepts a user-supplied proc alias or falls back to the source's
mutable filename. Required APIs, proc access and filesystem support must be
available. The macOS operation creates a separate clone; the Linux operation
creates a link to the held inode.

The macOS experiment explicitly prevents following a destination symlink.
Both a link to an existing file and a dangling link must be rejected with
`EEXIST`; neither the link nor its referent may change. The report records the
options passed to the API. There is no retry without the no-follow option.

The observations compare original bytes with the published file after replacing
the source name, reject occupied destinations and check a destination parent
whose visible name has changed. `source_selection_bound_to_fd` describes only
these executed observations. It does not promise a coherent snapshot during
concurrent writes to the held object. `production_ready` remains false.

This experiment can inform the physical representation of a retained revision.
It does not establish safe, complete restoration into a new directory, inspect
real deliveries, implement receipts, or satisfy Rust resource budgets. A file
result cannot be substituted for the directory-restoration contract.

## Run and interpret the report

On a native Linux or macOS host with Python 3.12:

```sh
python tools/directory_publication_probe.py --workspace "$PWD"
python tools/file_publication_identity_probe.py --workspace "$PWD"
```

The JSON report includes the platform, Python version, API, individual
observations and explicit failures or incomplete cases. Exit codes are:

| Code | Meaning |
|---|---|
| 0 | Every required synthetic observation matched its expectation. |
| 1 | An executed observation failed. |
| 2 | An unexpected error prevented the experiment from completing. |
| 3 | A required observation or safe cleanup could not be completed. |

The `directory-publication-probe` workflow runs on pull requests that change
these experiment files. Each native runner retains bounded JSON reports even
if the probe fails. The workflow has no publishing or release permissions.
Windows is outside this experiment; a Linux or macOS result cannot establish
Windows behavior. A report must be associated with the exact tested commit.

## API references

- [Linux rename and exclusive rename flags](https://man7.org/linux/man-pages/man2/renameat2.2.html).
- [Linux hard links and the directory restriction](https://man7.org/linux/man-pages/man2/linkat.2.html).
- [Linux descriptor aliases and access requirements](https://man7.org/linux/man-pages/man5/proc_pid_fd.5.html).
- [Apple clonefile and fclonefileat](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/clonefile.2).
- [Apple clone options](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/clonefile.h).
- [Apple destination lookup implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_syscalls.c).
- [Apple exclusive-renaming volume capability](https://developer.apple.com/documentation/foundation/urlresourcevalues/volumesupportsexclusiverenaming).
- [Apple syscall declarations](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/syscalls.master).
- [Python descriptor-based recursive removal](https://docs.python.org/3.12/library/shutil.html#shutil.rmtree).
