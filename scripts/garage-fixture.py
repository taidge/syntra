#!/usr/bin/env python3
"""Run an official Garage binary as an isolated loopback S3 test fixture.

Run under Linux/WSL with --directory in this workspace's metadata target.
Only the owned child process is stopped. Existing buckets/config are not used.
Credentials stay in the fixture directory, which must not be published.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess


def port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--data-directory", type=Path,
                        help="optional POSIX filesystem directory for state (for WSL DrvFS hosts)")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    data_directory = args.data_directory.resolve(strict=True) if args.data_directory else directory
    if any((data_directory / name).exists() for name in ("meta", "objects")):
        raise RuntimeError("refusing to reuse existing Garage fixture state")
    version = subprocess.check_output([str(binary), "--version"], text=True, timeout=5).strip()
    api_port, rpc_port = port(), port()
    while rpc_port == api_port:
        rpc_port = port()
    config = directory / "garage.toml"
    config.write_text(
        f'metadata_dir = "{data_directory / "meta"}"\ndata_dir = "{data_directory / "objects"}"\n'
        'db_engine = "sqlite"\nreplication_factor = 1\n'
        f'rpc_bind_addr = "127.0.0.1:{rpc_port}"\nrpc_public_addr = "127.0.0.1:{rpc_port}"\n'
        f'rpc_secret = "{secrets.token_hex(32)}"\n'
        f'[s3_api]\ns3_region = "garage"\napi_bind_addr = "127.0.0.1:{api_port}"\n',
        encoding="utf-8")
    environment = os.environ.copy()
    environment.update(GARAGE_CONFIG_FILE=str(config), GARAGE_DEFAULT_ACCESS_KEY="GK" + secrets.token_hex(16),
        GARAGE_DEFAULT_SECRET_KEY=secrets.token_hex(32), GARAGE_DEFAULT_BUCKET="office-test", RUST_LOG="warn")
    credentials = {"endpoint": f"http://127.0.0.1:{api_port}", "bucket": environment["GARAGE_DEFAULT_BUCKET"],
        "region": "garage", "access_key": environment["GARAGE_DEFAULT_ACCESS_KEY"],
        "secret_key": environment["GARAGE_DEFAULT_SECRET_KEY"]}
    credential_path = directory / "credentials.json"
    credential_path.write_text(json.dumps(credentials), encoding="utf-8")
    # WSL DrvFS can deny POSIX chmod; its Windows directory ACL remains the
    # boundary. These are freshly generated disposable fixture credentials.
    try:
        credential_path.chmod(0o600)
    except PermissionError:
        pass
    with (directory / "stdout.log").open("wb") as stdout, (directory / "stderr.log").open("wb") as stderr:
        process = subprocess.Popen([str(binary), "server", "--single-node", "--default-bucket"],
            cwd=directory, env=environment, stdout=stdout, stderr=stderr)
        evidence = {"binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "pid": process.pid, "fixture_pid": os.getpid(), "credentials_file": str(credential_path),
            "endpoint": credentials["endpoint"], "service": "real S3-compatible server, single node",
            "version": version}
        (directory / "fixture.json").write_text(json.dumps(evidence, indent=2), encoding="utf-8")
        print(json.dumps(evidence), flush=True)
        def stop(_signal, _frame):
            if process.poll() is None:
                process.terminate()
        signal.signal(signal.SIGTERM, stop)
        signal.signal(signal.SIGINT, stop)
        try:
            return process.wait()
        finally:
            stop(None, None)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    raise SystemExit(main())
