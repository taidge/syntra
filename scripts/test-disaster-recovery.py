#!/usr/bin/env python3
"""Rehearse a cold SQLite and local blob restore with an existing release ZIP.

Run `python scripts/test-disaster-recovery.py [--archive <release.zip>]` from
the repository. The fixture, backup, and result stay in Cargo's target directory.
For a real shared store, stop every writer and GC worker before taking a matching
DB/blob snapshot. This rehearsal exercises only the local storage backend.
"""

import argparse
from contextlib import closing, contextmanager
import hashlib
from http.client import HTTPConnection, HTTPException
import json
import os
from pathlib import Path, PurePosixPath
import secrets
import shutil
import socket
import sqlite3
import stat
import subprocess
import time
import uuid
from zipfile import ZipFile


DATABASE = "office.sqlite"
WAL = DATABASE + "-wal"
RESPONSE_LIMIT = 1024 * 1024


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def target_directory(workspace):
    for variable in ("CARGO_HOME", "CARGO_TARGET_DIR", "TEMP", "TMP", "TMPDIR"):
        value = os.environ.get(variable)
        if value:
            check_artifact_path(Path(value))
    result = subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=workspace, text=True, encoding="utf-8", timeout=30,
    )
    directory = Path(json.loads(result)["target_directory"])
    check_artifact_path(directory)
    return directory


def check_artifact_path(path):
    normalized = os.path.abspath(path).replace("\\", "/").casefold().removeprefix("//?/").rstrip("/") + "/"
    require(not normalized.startswith(("d:/buildcache/", "/mnt/d/buildcache/"))
            and "/.codex/build-targets/" not in normalized,
            "artifact discovery must not use a prohibited legacy build/cache path")


def archive_path(path):
    check_artifact_path(path)
    return path.resolve(strict=True)


def unpack_runtime(archive, destination, binary_name):
    destination.mkdir()
    with ZipFile(archive) as package:
        entries = package.infolist()
        require(len(entries) <= 20_000 and sum(entry.file_size for entry in entries) <= 2**31,
                "release ZIP exceeds the fixture extraction limit")
        seen = set()
        for entry in entries:
            name = entry.filename
            parts = PurePosixPath(name).parts
            require(parts and not name.startswith("/") and not any(ch in name for ch in "\\:\0")
                    and all(part.rstrip(" .") == part for part in parts)
                    and not stat.S_ISLNK(entry.external_attr >> 16),
                    "unsafe release ZIP entry")
            if entry.is_dir() or (name != binary_name and parts[0] not in {"config", "migrations", "static"}):
                continue
            key = "/".join(parts).casefold()
            require(key not in seen, "duplicate release ZIP entry")
            seen.add(key)
            path = destination.joinpath(*parts)
            path.parent.mkdir(parents=True, exist_ok=True)
            with package.open(entry) as source, path.open("xb") as file:
                shutil.copyfileobj(source, file)
    binary = destination / binary_name
    for path in (binary, destination / "config/default.toml", destination / "static/index.html",
                 destination / "migrations/025_atomic_persistence.sql"):
        require(path.is_file(), f"missing release runtime file {path.name}")
    if os.name != "nt":
        binary.chmod(0o755)
    return binary


class Client:
    def __init__(self, port):
        self.port = port

    def request(self, method, path, expected, body=None, content_type=None, token=None):
        headers = {}
        if content_type:
            headers["Content-Type"] = content_type
        if token:
            headers["Authorization"] = "Bearer " + token
        with closing(HTTPConnection("127.0.0.1", self.port, timeout=10)) as connection:
            connection.request(method, path, body, headers)
            response = connection.getresponse()
            require(response.status == expected, f"{method} {path} returned {response.status}, expected {expected}")
            result = response.read(RESPONSE_LIMIT + 1)
            require(len(result) <= RESPONSE_LIMIT, "fixture response exceeds its size limit")
            return result

    def json(self, method, path, expected, body=None, token=None):
        encoded = json.dumps(body).encode() if body is not None else None
        response = json.loads(self.request(method, "/api/v1" + path, expected, encoded,
                                           "application/json" if encoded else None, token))
        require(response.get("success") is True, f"{method} {path} failed")
        return response["data"]


def stop(process):
    if process.poll() is None:
        process.kill()
    return process.wait(timeout=15)


@contextmanager
def run_server(instance, binary):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    nonce = secrets.token_hex(16)
    probe = f"/recovery-{nonce}.txt"
    (instance / "static" / probe[1:]).write_text(nonce, encoding="ascii")
    (instance / "config/recovery-smoke.toml").write_text(
        f'[server]\nhost = "127.0.0.1"\nport = {port}\n'
        f'[database]\nurl = "sqlite://data/{DATABASE}?mode=rwc"\n'
        '[auth]\njwt_secret = "${JWT_SECRET}"\n'
        '[storage]\nbackend = "local"\nbase_path = "data/blobs"\n'
        '[maintenance]\nhistory_enabled = false\nreconcile_enabled = false\n'
        '[trash]\nauto_delete_enabled = false\n[rate_limit]\nenabled = false\n[ai]\nenabled = false\n',
        encoding="utf-8",
    )
    env = {key: value for key, value in os.environ.items()
           if not key.upper().startswith(("SYNTRA_", "\x43ONTRIX_")) and key.upper() not in {"APP_ENV", "JWT_SECRET"}}
    env.update(SYNTRA_ENV="recovery-smoke", JWT_SECRET=secrets.token_hex(32), RUST_LOG="warn")
    client = Client(port)
    with (instance / "stdout.log").open("wb") as stdout, (instance / "stderr.log").open("wb") as stderr:
        process = subprocess.Popen([str(binary)], cwd=instance, env=env, stdout=stdout, stderr=stderr,
                                   creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        try:
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                require(process.poll() is None, f"fixture exited during startup, see {instance / 'stderr.log'}")
                try:
                    ready = client.json("GET", "/ready", 200)
                    if ready["status"] == "ready" and client.request("GET", probe, 200) == nonce.encode():
                        break
                except (OSError, HTTPException, RuntimeError, ValueError):
                    pass
                time.sleep(0.2)
            else:
                raise RuntimeError(f"fixture did not become ready, see {instance / 'stderr.log'}")
            yield client, process
        finally:
            stop(process)


def download(client, file_id, token):
    return client.request("GET", f"/api/v1/files/{file_id}/download", 200, token=token)


def populate(client, login, username):
    client.json("POST", "/auth/register", 201, dict(login, username=username))
    token = client.json("POST", "/auth/login", 200, login)["token"]
    doc = client.json("POST", "/documents", 201, {"name": "Cold recovery", "document_type": "writer"}, token)
    path = f"/documents/{doc['id']}"
    for suffix in ("first", "last"):
        doc["content"]["title"] = f"Cold recovery {username} {suffix} 世界"
        result = client.json("PUT", path, 200, {"expected_version": doc["version"], "content": doc["content"]}, token)
        require(result["version"] == doc["version"] + 1, "save did not advance the document version")
        doc["version"] = result["version"]
    current = download(client, doc["file_id"], token)
    require(json.loads(current)["title"] == doc["content"]["title"], "source download omitted the latest save")
    versions = client.json("GET", path + "/versions", 200, token=token)
    require(len(versions) >= 2, "source document has no version history")

    original = secrets.token_bytes(4096)
    boundary = secrets.token_hex(16)
    multipart = (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="original.bin"\r\n'
                 f'Content-Type: application/octet-stream\r\n\r\n').encode() + original + f"\r\n--{boundary}--\r\n".encode()
    uploaded = json.loads(client.request("POST", "/api/v1/files/upload", 201, multipart,
                                         f"multipart/form-data; boundary={boundary}", token))
    require(uploaded.get("success") is True, "original file upload failed")
    original_id = uploaded["data"]["id"]
    require(download(client, original_id, token) == original, "source original download differs from uploaded bytes")
    return doc, versions, current, original_id, original


def inventory(directory, *, allow_live_index=False):
    """Hash each DB, WAL and blob file. SQLite rebuilds its disposable WAL index."""
    files = {}
    for path in directory.rglob("*"):
        metadata = path.lstat()
        is_reparse_point = getattr(metadata, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT
        require(not stat.S_ISLNK(metadata.st_mode) and not is_reparse_point,
                "fixture contains a link or reparse point")
        if path.is_dir():
            continue
        relative = path.relative_to(directory).as_posix()
        if allow_live_index and relative == DATABASE + "-shm":
            continue
        require(stat.S_ISREG(metadata.st_mode) and (relative in {DATABASE, WAL} or relative.startswith("blobs/")),
                f"unexpected backup data file {relative}")
        files[relative] = {"size": metadata.st_size, "sha256": sha256(path)}
    return files


def verify(directory, expected):
    actual = inventory(directory)
    return {"missing": sorted(expected.keys() - actual.keys()),
            "unexpected": sorted(actual.keys() - expected.keys()),
            "changed": sorted(key for key in actual.keys() & expected.keys() if actual[key] != expected[key])}


def copy_backup(source, destination, manifest):
    destination.mkdir()
    for name in manifest:
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / name, path)
    require(not any(verify(destination, manifest).values()), "copied backup does not match its manifest")


def rehearse(root, archive, binary_name, evidence):
    source, restored = root / "source", root / "restored"
    source_binary = unpack_runtime(archive, source, binary_name)
    evidence["binary_sha256"] = sha256(source_binary)
    username = "recovery_" + secrets.token_hex(8)
    login = {"email": username + "@example.test", "password": "Recovery!9-" + secrets.token_hex(16)}
    source_data = source / "data"
    with run_server(source, source_binary) as (client, process):
        # Pin a pre-write read snapshot so the cold copy must recover committed
        # WAL frames. The read-only connection closes before copying begins.
        uri = (source_data / DATABASE).as_uri() + "?mode=ro"
        with closing(sqlite3.connect(uri, uri=True)) as reader:
            reader.execute("BEGIN")
            reader.execute("SELECT COUNT(*) FROM documents").fetchone()
            doc, versions, current, original_id, original = populate(client, login, username)
            require(process.poll() is None, "source fixture exited before the controlled stop")
            evidence["source_exit_code"] = stop(process)

    manifest = inventory(source_data, allow_live_index=True)
    require(DATABASE in manifest and manifest.get(WAL, {}).get("size", 0) > 32, "source has no replayable WAL")
    blob_hashes = {name: entry["sha256"] for name, entry in manifest.items() if name.startswith("blobs/")}
    current_hash = hashlib.sha256(current).hexdigest()
    original_hash = hashlib.sha256(original).hexdigest()
    require(current_hash in blob_hashes.values(), "current document blob is absent from the physical snapshot")
    original_path = next((name for name, digest in blob_hashes.items() if digest == original_hash), None)
    require(original_path, "uploaded original is absent from the physical snapshot")
    backup = root / "backup"
    copy_backup(source_data, backup, manifest)
    manifest_file = root / "manifest.json"
    write_json(manifest_file, manifest)
    evidence.update(source_stopped_before_copy=True, manifest_sha256=sha256(manifest_file),
                    wal_bytes=manifest[WAL]["size"], blob_files=len(blob_hashes),
                    current_blob_sha256=current_hash, original_blob_sha256=original_hash)

    # Remove or mutate a backup blob in a disposable copy before any server
    # can open it. The same verifier gates the real restore below.
    broken = root / "negative-check"
    copy_backup(backup, broken, manifest)
    victim = broken / original_path
    victim.unlink()
    missing = verify(broken, manifest)
    require(missing == {"missing": [original_path], "unexpected": [], "changed": []}, "missing blob was not detected")
    victim.write_bytes(bytes([original[0] ^ 1]) + original[1:])
    changed = verify(broken, manifest)
    require(changed == {"missing": [], "unexpected": [], "changed": [original_path]}, "tampered blob was not detected")
    evidence.update(missing_blob_rejected=missing, tampered_blob_rejected=changed)

    db_only = root / "database-without-wal.sqlite"
    shutil.copyfile(backup / DATABASE, db_only)
    with closing(sqlite3.connect(db_only.as_uri() + "?immutable=1", uri=True)) as reader:
        has_schema = reader.execute("SELECT 1 FROM sqlite_schema WHERE name = 'documents'").fetchone()
        db_only_version = reader.execute("SELECT version FROM documents WHERE id = ?", (doc["id"],)).fetchone() if has_schema else None
    require(db_only_version is None, "the fixture cannot prove the WAL was necessary")

    restored_binary = unpack_runtime(archive, restored, binary_name)
    require(sha256(restored_binary) == evidence["binary_sha256"], "restored fixture server differs from source")
    restored_data = restored / "data"
    require(sha256(manifest_file) == evidence["manifest_sha256"], "backup manifest changed before restore")
    saved_manifest = json.loads(manifest_file.read_text(encoding="utf-8"))
    require(not any(verify(backup, saved_manifest).values()), "backup verification failed before restore")
    copy_backup(backup, restored_data, saved_manifest)
    uri = (restored_data / DATABASE).as_uri() + "?mode=ro"
    with closing(sqlite3.connect(uri, uri=True)) as reader:
        require(reader.execute("PRAGMA quick_check").fetchall() == [("ok",)], "restored SQLite integrity check failed")
    with run_server(restored, restored_binary) as (client, process):
        token = client.json("POST", "/auth/login", 200, login)["token"]
        path = f"/documents/{doc['id']}"
        recovered = client.json("GET", path, 200, token=token)
        require(recovered["version"] == doc["version"] and recovered["content"] == doc["content"],
                "restored document content/version differs from source")
        require(client.json("GET", path + "/versions", 200, token=token) == versions, "restored history differs from source")
        require(download(client, doc["file_id"], token) == current, "restored current blob differs from source")
        require(download(client, original_id, token) == original, "restored original differs from source")
        evidence["restored_exit_code"] = stop(process)
    require(not any(verify(backup, saved_manifest).values()), "running the restore changed the backup")
    evidence.update(restored_login=True, wal_replay_required=True, sqlite_quick_check=True,
                    restored_from_verified_manifest=True, backup_unchanged_after_restore=True,
                    restored_document_version=doc["version"], restored_history_entries=len(versions),
                    restored_document_blob=True, restored_uploaded_original=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, help="existing release ZIP, default: host ZIP under target/dist")
    args = parser.parse_args()
    workspace = Path(__file__).resolve().parent.parent
    target = target_directory(workspace)
    archive = args.archive
    if archive is None:
        version = subprocess.check_output(["rustc", "-vV"], cwd=workspace, text=True, timeout=30)
        host = next(line.split(": ", 1)[1] for line in version.splitlines() if line.startswith("host: "))
        archive = target / "dist" / f"syntra-{host}.zip"
    archive = archive_path(archive)
    root = target / "disaster-recovery" / uuid.uuid4().hex
    root.mkdir(parents=True)
    print(f"Disaster recovery artifacts: {root}", flush=True)
    evidence = {"success": False, "archive": str(archive), "archive_sha256": sha256(archive)}
    binary_name = "syntra-server.exe" if os.name == "nt" else "syntra-server"
    try:
        rehearse(root, archive, binary_name, evidence)
        require(sha256(archive) == evidence["archive_sha256"], "release ZIP changed during the rehearsal")
        evidence["success"] = True
        print("Disaster recovery passed.", flush=True)
    except Exception as error:
        evidence["error"] = str(error)
        print(f"Disaster recovery failed: {error}", flush=True)
    finally:
        write_json(root / "result.json", evidence)
    return 0 if evidence["success"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
