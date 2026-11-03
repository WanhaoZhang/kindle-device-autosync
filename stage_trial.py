#!/usr/bin/env python3
"""Stage a reviewed Kindle trial request. This tool makes no network requests."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile

HEX_SHA256 = re.compile(r"[0-9a-f]{64}")


def read_object(path):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 1024 * 1024:
        raise ValueError(f"Missing or unsafe JSON file: {path.name}")
    value = json.loads(path.read_bytes())
    if not isinstance(value, dict):
        raise ValueError(f"Expected a JSON object: {path.name}")
    return value


def write_request(mount, request):
    if mount.is_symlink() or not mount.is_dir() or not (mount / "documents").is_dir():
        raise ValueError("Select the mounted Kindle USB volume.")
    inbox = mount / ".kindle-native-sync-actions"
    inbox.mkdir(exist_ok=True)
    target = inbox / "request.json"
    if inbox.is_symlink() or target.is_symlink():
        raise ValueError("Unsafe action inbox.")
    fd, temp = tempfile.mkstemp(prefix=".request-", dir=inbox)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(request, stream, ensure_ascii=False)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, target)
    finally:
        Path(temp).unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mount", type=Path, default=Path("/Volumes/Kindle"))
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--book", required=True, help="Exact mapped local_book_id")
    prepare.add_argument("--accept-unverified-performance", required=True, action="store_true")
    confirm = commands.add_parser("confirm")
    confirm.add_argument("--plan-id", required=True, help="Exact plan ID you have reviewed")
    confirm.add_argument("--accept-unverified-protocol", required=True, action="store_true")
    args = parser.parse_args()
    out = args.mount / ".kindle-native-sync-export"
    config_inbox = args.mount / ".kindle-native-sync-inbox"
    request = {"schema_version": 1}
    try:
        if out.is_symlink() or config_inbox.is_symlink():
            raise ValueError("Unsafe USB configuration or export directory.")
        if args.command == "prepare":
            if config_inbox.is_dir():
                config = read_object(config_inbox / "config.json")
                if not any(row["local_book_id"] == args.book for row in config["mappings"]):
                    raise ValueError("The selected book is absent from the staged mappings.")
                del config["validation"]
                body = json.dumps(config, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
                binding = hashlib.sha256(body.encode("utf-8")).hexdigest()
            else:
                binding = read_object(out / "status.json")["binding_sha256"]
            request.update(action="authorize-prepare", local_book_id=args.book,
                           max_seconds=60, accept_unverified_performance=True)
        else:
            preview = read_object(out / "action-authorize-prepare.json")
            plan = preview.get("plan", {})
            if (preview.get("state") != "prepared" or not isinstance(plan, dict)
                    or plan.get("trial") is not True or plan.get("plan_id") != args.plan_id):
                raise ValueError("The USB preview does not match the reviewed trial plan.")
            binding = plan["binding_sha256"]
            if not HEX_SHA256.fullmatch(args.plan_id):
                raise ValueError("Invalid trial plan ID.")
            request.update(action="confirm-enable", plan_id=args.plan_id,
                           accept_unverified_protocol=True)
        if not isinstance(binding, str) or not HEX_SHA256.fullmatch(binding):
            raise ValueError("Invalid configuration binding.")
        request["config_binding_sha256"] = binding
        write_request(args.mount, request)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"{error}\n")
    print(f"Staged {request['action']}. Eject the Kindle and run the matching KUAL action.")


if __name__ == "__main__":
    main()
