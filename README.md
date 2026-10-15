# rusty-ld

rusty-ld is a fork of [mold](https://github.com/rui314/mold) 3.0.0, the Rust
rewrite of the mold linker (ELF linker for Linux, plus a Mach-O linker in the
same workspace). The command it builds is still called `mold`, and it links
with the same command line as GNU ld.

The fork keeps everything upstream does and adds a small number of things
that were missing for our own use. This page says what those are, how the
fork compares with the code it started from, how to build and install it, and
where it falls short. For the linker's general documentation (every option,
the man page, supported targets) read `docs/mold.md` and the upstream project.

## Why this fork exists

We needed a fast linker that could build a C library whose link step depends
on behaviour GNU ld has and mold 3.0.0 lacked (a section ordering file,
PLT marking, symbols that only a shared library defines, ifunc ordering in
static programs). We also wanted the linker to explain itself to scripts and
build tools: which library holds a missing symbol, where each definition of a
duplicate symbol lives, why a section was kept, in a format a program can parse
without scraping error text. Upstream is a general-purpose linker, so these
live here as opt-in additions that leave normal links alone.

## What is different from upstream

Linking behaviour (all on by default or opt-in as noted; each came with tests):

- `--section-ordering-file FILE`: place input sections at the start of their
  output sections in the order a file lists them.
- `-z mark-plt` / `-z nomark-plt` (x86-64, off by default): add the dynamic tags
  that let glibc 2.39 or later rewrite PLT entries into direct jumps, and
  put the PLT entry address in each jump-slot relocation.
- Symbols named with `-u` or `--require-defined` that only a shared library
  defines are now imported (`.dynsym` and `.gnu.version_r`), as GNU ld and lld
  do.
- `-z noseparate-relro` (opt-in): keep RELRO and the other writable data in one
  segment instead of padding to a page boundary.
- Output ordering closer to GNU ld and lld for `.gnu.version_d`, `.relr.dyn`,
  merged constant sections and unwind tables.
- In static executables, IRELATIVE relocations are ordered so an ifunc
  resolver that calls another ifunc function (for example `strlen`, `memcpy`
  from the C library) runs after that function is resolved.
- A copy relocation is sized by the largest alias of the symbol, which fixes
  old versioned data such as `sys_errlist`.

Agent mode (opt-in, off by default): machine-readable output for scripts and
tools that read the linker's output. See [Agent mode](#agent-mode).

What is upstream's and what is ours: the linker itself, all other options,
the 20 ELF targets and the Mach-O linker, the test runner and the man page are
upstream's. The list above, the code behind agent mode (`elf/agent.rs` and its
hooks in the driver, command-line parser and reports) and the tests named
`tests/elf/agent-*.sh` are ours. With agent mode off, links and messages are
meant to be unchanged from the code we forked from.

## How it compares

All comparisons below are against our own previous commit, not against
upstream mold. We have no recorded head-to-head measurement against upstream
mold 3.0.0, so this page makes no claim about speed or size relative to it.
Upstream publishes its own benchmarks; see its project page.

| What | Result | Method and source |
|---|---|---|
| Link speed with agent features present but off | Equal within noise: default link 175.7 ms before, 176.5 ms after (1.004x); `--gc-sections --icf=all` 172.7 before, 173.8 after (1.006x). A 0.4 to 0.6 percent slowdown, so not faster. | Link of 4,001 synthetic objects through the clang driver, interleaved runs of both builds, median of 16 each, 2026-10-14. Machine class not recorded. |
| Link time (`--perf`) when all agent features were first added | 0.0685 s after against 0.069 s before | 80 alternating runs, median, 2026-10-13. Machine class not recorded. |
| Output files with agent mode off | Same bytes (hashes of the hello-world link, the `--gc-sections --icf` link and both 4,001-object links match) | Hash comparison against the previous commit, 2026-10-14. |
| Binary size (x86-64-only release build) | Larger by about 300 KB unstripped (5,208,112 to 5,508,240 bytes) and 252 KB stripped (4,008,064 to 4,260,536 bytes) after the first agent features. About 164 KB of that is the DWARF reader used to find source lines. | Sizes recorded when the features were added, 2026-10-13. Later agent-mode work was not re-measured for size. |

Where we are slower, equal or have not measured:

- Slower: the binary is bigger (above), and agent mode itself costs time when
  switched on (it reads DWARF for line numbers and holds messages to sort them);
  we did not record how much for a large link.
- Equal: link speed with the new features off (above).
- Not measured: speed and size against upstream mold 3.0.0 or against GNU ld,
  gold and lld; targets other than x86-64.

## Test results

The integration suite is upstream's runner (`tests/`) with our tests added.
On the x86-64 target it was run with agent mode off (`RUSTY_AGENT` must be
unset, see below).

`env -u RUSTY_AGENT cargo test --release -p mold --test integration -- --native` on a fresh
clone of this repository (2026-10-15): **545 passed, 47 skipped, 1 failed** (593 tests). The
skipped tests skip themselves (the suite decides per test; we did not break down why). The
failing test is `lto-unclaimed`: it links with `clang -flto`, and the installed LLVM 22 linker
plugin answers "Invalid bitcode signature". The notes record that it failed the same way on our fork before the
agent-mode changes, so it is not caused by them; we did not run it against upstream's own tree.

Reference points from the project notes: before the 2026-10-14 agent-mode fixes the same suite
gave 537 passed, 47 skipped, 1 failed (the same test); the tests added since are ours
(`tests/elf/agent-*.sh` and others for the options listed above). We have no recorded test
count for upstream's own tree at the point we forked. Only x86-64 was run. The suite must be run
with `RUSTY_AGENT` unset: with it set to 1 many tests see JSON where they expect text and fail.

## Agent mode

Agent mode is a feature for scripts and tools that read the linker's output.
It is off unless you ask for it: `--agent-mode` on the command line, or
`RUSTY_AGENT=1` in the environment. `--no-agent-mode` turns it off again, wherever
it appears. With it off, nothing below is read, opened or written.

- Switches on `--explain-undefined` (which library defines a missing symbol,
  near-miss names, source lines of the references), `--explain-duplicates`
  (source file and line of each definition), `--diagnostics-format=json` and
  `--stable-diagnostics` (messages held back and sorted, so two runs print the
  same text in the same order).
- Output format: one JSON object per line, each starting with `"schema": 1` and a
  `"type"` (`diagnostic`, `size-report`, `why`, `link-summary`, `command-line`).
  Diagnostics carry stable codes (`L0001` undefined symbol, `L0002` duplicate
  symbol, and so on) that do not depend on the message text.
- Extra reports, each also usable alone: `--size-report`, `--why-kept` and
  `--why-removed` (the reference chain behind a `--gc-sections` or `--icf`
  decision), `--link-summary=FILE`, `--echo-command-line`.
- Detail levels: `--agent-detail=brief|normal|full` or `RUSTY_AGENT_DETAIL`;
  `normal` is the default. At `normal` and `brief` a size budget of 12 KB soft
  and 20 KB hard applies to records on the error stream, with a final
  `records-omitted` record saying what was left out.
- Output channels: records go to standard error as always, and agent mode can
  send a second clean copy to a file (`RUSTY_AGENT_OUT=path`) or an open
  descriptor (`RUSTY_AGENT_FD=n`) so they do not get mixed with other text when a
  caller merges the two streams. A bad value is a warning, never a failure.
- `--agent-help` prints the options, record types, fields and codes as JSON.

Example (one undefined symbol; `mold` here is the program built from this
repository; the plain-text message is what you get without the variable):

    $ RUSTY_AGENT=1 clang -fuse-ld=mold u.o -o u
    {"schema":1,"type":"diagnostic","code":"L0001","severity":"error","message":"undefined symbol: missing_fn","locations":[{"object":"u.o","file":"u.c","function":"main"}]}

    $ clang -fuse-ld=mold u.o -o u
    mold: error: undefined symbol: missing_fn
    >>> referenced by u.c
    >>>               u.o:(main)

Full description: the "Agent mode" section of [docs/mold.md](docs/mold.md).

## Install

The fork installs the same names as upstream: `mold`, `ld.mold`, `ld64.mold`,
and the `ld` link GCC finds with `-B`. From source the default prefix is
`/usr/local`, so it sits beside the system linker; GNU ld (`/usr/bin/ld`), gold
and lld are not replaced. If a distribution package for `mold` is installed under `/usr/bin`, our package-style install
would replace it, so use one or the other in the same prefix.

Use it by name, per build, with no system change:

    clang -fuse-ld=mold main.o -o main
    gcc -B/usr/local/libexec/mold main.o -o main    # the `ld` link in that directory is mold

(`-fuse-ld=mold` with clang, and `-B` with gcc, were tried on a hello-world link
with the freshly built binary.)

## Build and test

Needs a Rust toolchain (edition 2024, `rust-version` 1.95 in `Cargo.toml`) and a
C compiler for the test programs. x86-64 Linux is what we build and test.

    cargo build --release --locked
    ./install-mold.sh                      # PREFIX=/usr/local by default, DESTDIR supported
    env -u RUSTY_AGENT cargo test --release -p mold --test integration -- --native

The release build compiles every target and took 4 minutes 7 seconds with three
jobs on a 12-thread machine (2026-10-15, fresh clone). The packaging notes record
about 4 GB of memory at the default job count, so use `-j 2` on small machines.
Some tests need `clang` and `gcc`; `install-test-deps.sh` installs the packages the tests need.

## Known limits

- Only x86-64 is built and tested here. The other 19 ELF targets and the Mach-O
  linker come from upstream and were not rebuilt or tested by us.
- `-z mark-plt` is x86-64 only and does nothing useful without a glibc that
  supports PLT rewriting.
- Agent mode: line numbers for references need a 64-bit RELA object compiled with
  line tables; 32-bit and REL targets (i386, arm32) get none. At most 64 object
  files are read for lines per link. Records are a draft format: fields can be
  added without changing `schema`.
- `--diagnostics-format` only changes messages printed after it on the command
  line; `RUSTY_AGENT=1` is read before everything.
- A bad `--agent-detail` is an error; a bad `RUSTY_AGENT_DETAIL` is ignored.
- Link-time optimisation through the linker plugin depends on your LLVM version
  (see the failing test noted under test results).

## Licence and credit

MIT, as upstream (see `LICENSE`). mold was created by Rui Ueyama and its
contributors; their copyright notice stays in `LICENSE`. The additions in this
fork are released under the same licence. This is a personal fork and is not
affiliated with, or endorsed by, the upstream project.
