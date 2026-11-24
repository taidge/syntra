# Repeatable capacity and integration acceptance

These tools record measured workload parameters and failures. They do not imply
a hardware-independent latency SLO or Amazon S3 production acceptance.

## HTTP latency, overload, memory and document isolation

Build the binary through the workspace's normal configuration, then run:

```powershell
./scripts/benchmark.ps1 -Workers 2 -RequestsPerWorker 100 -HotExports 30
./scripts/benchmark.ps1 -Workers 8 -RequestsPerWorker 100 -HotExports 30 -HotTitleBytes 2097152
```

The wrapper discovers Cargo's configured target through metadata and does not
build. It delegates to `scripts/benchmark-http.py`, which copies the selected
release binary into a unique output-adjacent instance, isolates application
configuration/database/storage, binds loopback, disables proxies only on its
test HTTP client, and stops only its owned server. Cargo/TEMP settings and
machine proxy settings are inherited unchanged.

Each worker owns an independent Writer. The measured mix is 90% GET and 10% CAS
PUT, preceded by five unmeasured GETs per document. No failed operation is
automatically retried: failed CAS writes trigger an explicitly unmeasured state
refresh so one transport error does not manufacture subsequent stale writes.
The result contains successful latency p50/p95/p99, attempted and successful
throughput, and every failed workload/resync request (including 429/503 or I/O
errors). If an HTTP failure has a valid `X-Request-Id`, the result records it
for correlation with the server's overload log. The tool exits unsuccessfully
when errors occurred.

The second scenario compares independent cold-document GETs before and during
a fixed count of hot-document JSON exports. Cold sampling stops when the hot
exports finish, so the loaded distribution excludes a long idle tail. Actual
sample counts are recorded; interpret percentiles with that sample count.

Server RSS is sampled using GetProcessMemoryInfo, `/proc`, or `ps` every 50 ms.
The sampled peak is not an allocator measurement or a guaranteed OS high-water
mark. Record background builds, power settings and host load alongside results.
Binary SHA256/mtime, platform, CPU count, raw RSS samples and all load parameters
are included in each JSON result. Keep before/after artifacts and instance logs.
The checked-in `baselines/windows-before-maintenance-2026-11-10.json` is a
descriptive result from the earlier release binary, with its exact SHA256. It
does not validate subsequent changes, and its host background load was not
controlled. The raw artifact path identifies the complete original sample set.

## Formula work and sparse XLSX export

```powershell
cargo test --locked --release -p syntra-server --test capacity_benchmarks -- --ignored --nocapture --test-threads=1
```

The ignored test measures 30 samples for each of 100/1000/5000 independent
four-cell formula diamonds, validates each result, and emits calculation-work
counters. The sparse export cases both contain 1001 populated cells, with last
coordinates `(999,9)` and `(1048575,16383)`. Export timing excludes reimport;
every generated workbook is reimported to verify the edge value. Each JSON line
states debug/release profile. Debug measurements must not be described as release
capacity results. Avoid concurrent build/benchmark runs for comparative evidence.

## Real S3-compatible service

`crates/server/tests/real_s3_acceptance.rs` is ignored by default and requires
an independently running service and a disposable bucket. It checks a correct
SigV4 credential, rejection of an incorrect secret, binary byte equality, HEAD,
UTF-8/space/plus/percent/fragment/query characters in keys, missing-object reads,
deletion and idempotent deletion. It creates only a fresh UUID prefix.

Pass `SYNTRA_S3_ENDPOINT`, `SYNTRA_S3_BUCKET`, `SYNTRA_S3_ACCESS_KEY`,
`SYNTRA_S3_SECRET_KEY` and `SYNTRA_S3_REGION` to the test process. Do not place
credentials in tracked source, shell history, screenshots or reports. For a
loopback fixture on a machine whose system proxy intercepts localhost, set
`NO_PROXY=127.0.0.1,localhost` only in that child test process's environment.
The application S3 client retains the caller's normal proxy behavior.

```text
cargo test --locked -p syntra-server --test real_s3_acceptance -- --ignored --nocapture --test-threads=1
cargo test --locked -p syntra-server --test maintenance_regressions live_s3_maintenance -- --ignored --nocapture --test-threads=1
```

`scripts/garage-fixture.py` can launch an official Garage Linux binary with an
isolated loopback service and generated disposable credentials. Supply a new
directory under the workspace metadata target; a POSIX `--data-directory` is
optional for WSL DrvFS hosts that reject chmod. If using a project-owned ext4
image, verify its absolute path and bound its size before mounting, then stop
the owned Garage process and unmount its exact mountpoint after acceptance.
No Docker builder or existing disk image is required or modified.

Before compiling application tests, an independent client can verify that the
real fixture enforces signatures and supports encoded keys and paginated LIST:

```text
python scripts/s3-preflight.py --credentials <private-fixture-directory>/credentials.json --output <artifact-directory>/s3-preflight.json
```

This is service preflight evidence; it does not replace Rust application-path
acceptance or establish Amazon S3 production compatibility.

## Native platform matrix

Run these on the actual target OS, from this same source revision and its pinned
submodules. Cross-compilation alone does not prove filesystem/network behavior.

| Host | Executable acceptance | Current environment availability |
| --- | --- | --- |
| Windows | workspace tests/Clippy, WASM build, package smoke, raw upload rejection and junction tests | Local native toolchain available |
| Linux | workspace tests/Clippy, WASM build, package smoke, raw upload rejection and symlink tests | Native Ubuntu WSL2 validation workspace available |
| macOS | workspace tests/Clippy, WASM build, package smoke and symlink tests | CI and release use `macos-15-intel` to validate the Intel ZIP; actual runs require the matching revision on a macOS runner |

On hosts with PowerShell, run `./scripts/check.ps1 -BuildWeb`, then
`./scripts/package.ps1` and `./scripts/test-package.ps1` with the generated archive
as shown by the package script's output and the smoke script's parameters. For
Linux without PowerShell, the core native gate is:

```sh
cargo metadata --no-deps --format-version 1
cargo test --locked --workspace --features syntra-frontend/ssr
cargo clippy --locked --workspace --all-targets --no-deps --features syntra-frontend/ssr -- -D warnings
cargo check --locked -p syntra-frontend --lib --target wasm32-unknown-unknown
```

For a container image already built with an eligible Docker builder, run
`./scripts/test-package.ps1 -Image syntra -BrowserTests`. It starts the
production image on a loopback port with an isolated volume and checks assets,
browser saving, persistence after SIGKILL, and WebSocket replay.

Respect the caller's Cargo and temporary-directory configuration. Do not invoke
retired cache aliases, mutate global proxy settings, push a branch, trigger paid
resources or label a prepared matrix as executed platform acceptance.
