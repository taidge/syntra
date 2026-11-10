#!/usr/bin/env python3
"""Independent SigV4 preflight for a disposable real S3-compatible bucket.

This proves the fixture's authentication/encoding/pagination behavior. The Rust
real_s3_acceptance and maintenance tests separately verify application code.
Credentials are read from a private JSON fixture file and are never printed.
"""
import argparse
import datetime
import hashlib
import hmac
import json
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request
import uuid
import xml.etree.ElementTree as ET


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def signed(credentials, method, key="", data=b"", query=None, wrong_secret=False):
    endpoint = credentials["endpoint"].rstrip("/")
    path = "/" + urllib.parse.quote(credentials["bucket"], safe="") + "/" + urllib.parse.quote(key, safe="/-_.~")
    query = urllib.parse.urlencode(sorted((query or {}).items()), quote_via=urllib.parse.quote)
    host = urllib.parse.urlsplit(endpoint).netloc
    timestamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    date = timestamp[:8]
    payload_hash = hashlib.sha256(data).hexdigest()
    headers = {"host": host, "x-amz-content-sha256": payload_hash, "x-amz-date": timestamp}
    header_names = ";".join(sorted(headers))
    canonical_headers = "".join(name + ":" + headers[name] + "\n" for name in sorted(headers))
    canonical = "\n".join([method, path, query, canonical_headers, header_names, payload_hash])
    scope = f"{date}/{credentials['region']}/s3/aws4_request"
    string_to_sign = "AWS4-HMAC-SHA256\n" + timestamp + "\n" + scope + "\n" + hashlib.sha256(canonical.encode()).hexdigest()
    def mac(key_bytes, value):
        return hmac.new(key_bytes, value.encode(), hashlib.sha256).digest()
    secret = "invalid-secret" if wrong_secret else credentials["secret_key"]
    signing_key = ("AWS4" + secret).encode()
    for piece in (date, credentials["region"], "s3", "aws4_request"):
        signing_key = mac(signing_key, piece)
    signature = hmac.new(signing_key, string_to_sign.encode(), hashlib.sha256).hexdigest()
    headers["authorization"] = f"AWS4-HMAC-SHA256 Credential={credentials['access_key']}/{scope}, SignedHeaders={header_names}, Signature={signature}"
    url = endpoint + path + ("?" + query if query else "")
    request = urllib.request.Request(url, data=data if method == "PUT" else None, headers=headers, method=method)
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=10) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--credentials", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    credentials = json.loads(args.credentials.read_text(encoding="utf-8"))
    prefix = "independent-preflight/" + uuid.uuid4().hex + "/"
    keys = [prefix + "plain.bin", prefix + "空 格+%#?.bin"]
    payload = bytes(index % 251 for index in range(65537))
    results = []
    errors = []
    try:
        status, _ = signed(credentials, "HEAD")
        require(status == 200, f"authenticated HEAD bucket: {status}")
        status, _ = signed(credentials, "HEAD", wrong_secret=True)
        require(status in (401, 403), f"invalid signature accepted or wrong server: {status}")
        results.append("valid signature accepted; invalid signature rejected")
        for key in keys:
            status, _ = signed(credentials, "PUT", key, payload)
            require(status in (200, 201), f"PUT: {status}")
            status, actual = signed(credentials, "GET", key)
            require(status == 200 and actual == payload, f"GET binary mismatch/status: {status}")
            status, _ = signed(credentials, "HEAD", key)
            require(status == 200, f"HEAD object: {status}")
        results.append("binary roundtrip and UTF8/reserved path encoding")
        listed = []
        token = None
        for _ in range(5):
            query = {"list-type": "2", "max-keys": "1", "encoding-type": "url", "prefix": prefix}
            if token:
                query["continuation-token"] = token
            status, body = signed(credentials, "GET", query=query)
            require(status == 200, f"LIST: {status}")
            root = ET.fromstring(body)
            local = lambda name: root.findall(".//{*}" + name)
            listed.extend(urllib.parse.unquote(item.text or "") for item in local("Key"))
            next_tokens = local("NextContinuationToken")
            token = next_tokens[0].text if next_tokens else None
            if not token:
                break
        require(sorted(listed) == sorted(keys), "paginated LIST missing/duplicating keys")
        results.append("ListObjectsV2 max-keys=1 and continuation token")
    except Exception as error:
        errors.append(f"{type(error).__name__}: {error}")
    finally:
        for key in keys:
            try:
                status, _ = signed(credentials, "DELETE", key)
                require(status in (200, 204), f"DELETE: {status}")
                status, _ = signed(credentials, "HEAD", key)
                require(status == 404, f"deleted HEAD: {status}")
            except Exception as error:
                errors.append("cleanup: " + str(error))
        evidence = {"service": "real S3-compatible service; independent Python SigV4 client",
                    "endpoint": credentials["endpoint"], "bucket": credentials["bucket"],
                    "objects": len(keys), "bytes_per_object": len(payload), "checks": results,
                    "errors": errors, "success": not errors}
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(evidence, ensure_ascii=False))
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
