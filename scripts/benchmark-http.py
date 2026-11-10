#!/usr/bin/env python3
"""Repeatable loopback HTTP load against an isolated copy of a server binary.

No compiler, global cache, proxy setting, or external account is changed. Keep
the output JSON and instance directory together when comparing two binaries.
"""
import argparse
import concurrent.futures
import copy
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import secrets
import shutil
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request
import uuid


def request(base, method, path, token=None, body=None, timeout=30):
    headers = {"Accept": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    data = None if body is None else json.dumps(body).encode()
    if data is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=data, headers=headers, method=method)
    # An isolated loopback fixture must not inherit system HTTP proxies.
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=timeout) as response:
            payload = json.loads(response.read())
            return payload.get("data", payload)
    except urllib.error.HTTPError as error:
        text = error.read(4096).decode("utf-8", errors="replace")
        raise RuntimeError(f"{method} {path}: HTTP {error.code}, body={text!r}") from error


def failure_correlation(error):
    source = error.__cause__
    if not isinstance(source, urllib.error.HTTPError) or source.headers is None:
        return {}
    value = source.headers.get("X-Request-Id")
    if not value:
        return {}
    try:
        normalized = str(uuid.UUID(value))
    except ValueError:
        return {}
    if normalized != value.lower():
        return {}
    return {"request_id": normalized}


def percentiles(samples):
    values = sorted(samples)
    if not values:
        return {"count": 0, "p50_ms": None, "p95_ms": None, "p99_ms": None, "max_ms": None}
    return {**{f"p{p}_ms": values[max(0, math.ceil(len(values) * p / 100) - 1)]
               for p in (50, 95, 99)}, "count": len(values), "max_ms": values[-1]}


def resident_bytes(pid):
    if os.name == "nt":
        size = ctypes.c_size_t
        class Counters(ctypes.Structure):
            _fields_ = [("cb", ctypes.c_ulong), ("faults", ctypes.c_ulong)] + [
                (name, size) for name in ("peak_rss", "rss", "peak_paged", "paged",
                                         "peak_nonpaged", "nonpaged", "pagefile", "peak_pagefile", "private")]
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        psapi.GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.POINTER(Counters), ctypes.c_ulong]
        handle = kernel.OpenProcess(0x410, False, pid)
        if not handle:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            counters = Counters()
            counters.cb = ctypes.sizeof(counters)
            if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                raise ctypes.WinError(ctypes.get_last_error())
            return counters.rss
        finally:
            kernel.CloseHandle(handle)
    if platform.system() == "Linux":
        return int(Path(f"/proc/{pid}/statm").read_text().split()[1]) * os.sysconf("SC_PAGE_SIZE")
    # ps is an OS observation, not a replacement allocator or process launcher.
    return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True).strip()) * 1024


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument("--requests-per-worker", type=int, default=100)
    parser.add_argument("--hot-exports", type=int, default=30)
    parser.add_argument("--hot-title-bytes", type=int, default=32 * 1024)
    args = parser.parse_args()
    if min(args.workers, args.requests_per_worker, args.hot_exports, args.hot_title_bytes) < 1:
        parser.error("all load parameters must be positive")
    root = Path(__file__).resolve().parent.parent
    source = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    instance = output.parent / ("http-instance-" + uuid.uuid4().hex)
    instance.mkdir()
    binary = instance / source.name
    shutil.copy2(source, binary)
    shutil.copytree(root / "config", instance / "config")
    shutil.copytree(root / "migrations", instance / "migrations")
    (instance / "config" / "development.toml").write_text(
        '[server]\nhost="127.0.0.1"\n[rate_limit]\nenabled=false\n[ai]\nenabled=false\n'
        '[storage]\nbackend="local"\nbase_path="data/files"\n', encoding="utf-8")
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    environment = {key: value for key, value in os.environ.items()
                   if not key.upper().startswith("CONTRIX_") and key.upper() not in ("APP_ENV", "JWT_SECRET")}
    environment.update(CONTRIX_ENV="development", CONTRIX_SERVER_PORT=str(port),
                       CONTRIX_DATABASE_URL="sqlite://data/benchmark.db", RUST_LOG="warn")
    result = {"schema": 1, "started_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "binary": str(source), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "binary_mtime_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(source.stat().st_mtime)),
              "host": platform.platform(), "logical_cpus": os.cpu_count(), "instance": str(instance),
              "load": {"workers": args.workers, "requests_per_worker": args.requests_per_worker,
                       "mix": "90% GET /documents/:id, 10% CAS PUT; one independent document per worker",
                       "hot_exports": args.hot_exports, "hot_title_bytes": args.hot_title_bytes,
                       "warmup_reads_per_document": 5},
              "transport": "HTTP/1 loopback; fresh urllib connection per request; system proxies disabled"}
    stdout = (instance / "stdout.log").open("wb")
    stderr = (instance / "stderr.log").open("wb")
    process = subprocess.Popen([str(binary)], cwd=instance, env=environment, stdout=stdout, stderr=stderr,
                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    stop = threading.Event()
    memory = []
    errors = []
    def sample():
        while not stop.wait(0.05):
            try:
                memory.append({"at_s": time.perf_counter(), "rss_bytes": resident_bytes(process.pid)})
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                errors.append("memory sampler: " + str(error))
                return
    sampler = threading.Thread(target=sample, daemon=True)
    base = f"http://127.0.0.1:{port}/api/v1"
    try:
        ready_deadline = time.monotonic() + 15
        for _ in range(100):
            if process.poll() is not None:
                raise RuntimeError("server exited; inspect " + str(instance))
            if time.monotonic() >= ready_deadline:
                raise RuntimeError("server readiness timed out")
            try:
                request(base, "GET", "/ready", timeout=1)
                break
            except (OSError, RuntimeError, urllib.error.URLError):
                time.sleep(0.1)
        else:
            raise RuntimeError("server readiness timed out")
        registration = request(base, "POST", "/auth/register", body={"username": "bench_" + uuid.uuid4().hex[:12],
            "email": uuid.uuid4().hex + "@benchmark.test", "password": "BenchA!9" + secrets.token_hex(12)})
        token = registration["token"]
        docs = [request(base, "POST", "/documents", token,
                        {"name": f"Worker {index}", "document_type": "writer"}) for index in range(args.workers)]
        for doc in docs:
            for _ in range(5):
                request(base, "GET", "/documents/" + doc["id"], token)
        sampler.start()
        def worker(doc):
            timings = {"get": [], "put": []}
            failures = []
            content = copy.deepcopy(doc["content"])
            version = doc["version"]
            for index in range(args.requests_per_worker):
                method = "PUT" if index % 10 == 9 else "GET"
                body = None
                if method == "PUT":
                    content["title"] = "Benchmark revision " + str(index)
                    body = {"content": content, "expected_version": version}
                started = time.perf_counter()
                try:
                    reply = request(base, method, "/documents/" + doc["id"], token, body)
                    timings[method.lower()].append((time.perf_counter() - started) * 1000)
                    if method == "PUT":
                        version = reply["version"]
                except Exception as error:
                    failures.append({"method": method, "at_request": index,
                        "elapsed_ms": (time.perf_counter() - started) * 1000,
                        "error": f"{type(error).__name__}: {error}", **failure_correlation(error)})
                    # A timed-out write may still commit. Resync explicitly;
                    # this is outside the timed workload and never retries it.
                    if method == "PUT":
                        try:
                            current = request(base, "GET", "/documents/" + doc["id"], token)
                            version, content = current["version"], current["content"]
                        except Exception as refresh_error:
                            failures.append({"method": "RESYNC", "error": str(refresh_error),
                                             **failure_correlation(refresh_error)})
            return timings, failures
        started = time.perf_counter()
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as executor:
            measured = list(executor.map(worker, docs))
        elapsed = time.perf_counter() - started
        failures = [failure for _, items in measured for failure in items]
        successes = sum(len(group[method]) for group, _ in measured for method in ("get", "put"))
        result["http_mixed"] = {"elapsed_s": elapsed, "attempted_requests_per_second": args.workers * args.requests_per_worker / elapsed,
            "successful_requests_per_second": successes / elapsed, "failures": failures,
            "failed_workload_requests": sum(item["method"] != "RESYNC" for item in failures),
            **{method: percentiles([value for group, _ in measured for value in group[method]]) for method in ("get", "put")}}
        if failures:
            errors.append(f"mixed workload recorded {len(failures)} request/resync failures; see http_mixed.failures")
        # A large Writer JSON export creates repeated serialization/response work
        # in one document while another document receives lightweight reads.
        hot = request(base, "POST", "/documents", token, {"name": "Hot export", "document_type": "writer"})
        hot_content = copy.deepcopy(hot["content"])
        hot_content["title"] = "x" * args.hot_title_bytes
        request(base, "PUT", "/documents/" + hot["id"], token,
                {"content": hot_content, "expected_version": hot["version"]})
        def cold_reads(hot_done=None):
            samples = []
            for _ in range(100):
                if hot_done is not None and hot_done.is_set():
                    break
                start = time.perf_counter()
                request(base, "GET", "/documents/" + docs[0]["id"], token)
                samples.append((time.perf_counter() - start) * 1000)
            return percentiles(samples)
        isolated = cold_reads()
        hot_done = threading.Event()
        def hot_exports():
            try:
                for _ in range(args.hot_exports):
                    request(base, "POST", "/documents/" + hot["id"] + "/export/json", token)
            finally:
                hot_done.set()
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
            hot_future = executor.submit(hot_exports)
            cold_future = executor.submit(cold_reads, hot_done)
            loaded = cold_future.result()
            hot_future.result()
        result["cross_document"] = {"hot_document_title_bytes": len(hot_content["title"]),
            "hot_workload": "serial POST export/json", "cold_workload": "up to 100 GETs of an independent Writer, stop after hot exports finish",
            "cold_baseline": isolated, "cold_while_hot": loaded}
        result["success"] = not errors
    except Exception as error:
        result["success"] = False
        errors.append(f"{type(error).__name__}: {error}")
        correlation = failure_correlation(error)
        if correlation:
            result["terminal_request_id"] = correlation["request_id"]
    finally:
        stop.set()
        if sampler.is_alive():
            sampler.join(timeout=2)
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        stdout.close()
        stderr.close()
        result["memory"] = {"method": "server process RSS sampled every 50ms", "samples": memory,
            "sampled_peak_rss_bytes": max((item["rss_bytes"] for item in memory), default=None)}
        result["errors"] = errors
        output.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({key: value for key, value in result.items() if key != "memory"}, ensure_ascii=False))
    return 0 if result.get("success") and not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
